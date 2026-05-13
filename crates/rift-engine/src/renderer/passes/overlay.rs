use anyhow::Result;
use ash::vk;
use bytemuck::{Pod, Zeroable};
use std::sync::{Arc, RwLock};

use crate::hot_reload;
use crate::renderer::font::BitmapFont;
use crate::vulkan::buffer::{self, GpuBuffer};
use crate::vulkan::sync::MAX_FRAMES_IN_FLIGHT;

/// Shared `name -> [u0,v0,u1,v1]` icon registry. Owned by
/// `OverlayRenderer` (which mutates it as PNGs stream in during
/// loading) and read by `OverlayBatch::icon` at draw time. The
/// shared handle lets the batch see new icons without an
/// explicit hand-off after each load step.
pub type IconUvRegistry = Arc<RwLock<std::collections::HashMap<String, [f32; 4]>>>;

/// A 2D vertex for the overlay (screen-space NDC).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct OverlayVertex {
    pub position: [f32; 2], // NDC coords: -1..1
    pub color: [f32; 4],    // RGBA
    pub uv: [f32; 2],       // Texture coords (font atlas)
}

impl OverlayVertex {
    pub fn binding_description() -> vk::VertexInputBindingDescription {
        vk::VertexInputBindingDescription {
            binding: 0,
            stride: std::mem::size_of::<Self>() as u32,
            input_rate: vk::VertexInputRate::VERTEX,
        }
    }

    pub fn attribute_descriptions() -> [vk::VertexInputAttributeDescription; 3] {
        [
            vk::VertexInputAttributeDescription {
                binding: 0,
                location: 0,
                format: vk::Format::R32G32_SFLOAT,
                offset: 0,
            },
            vk::VertexInputAttributeDescription {
                binding: 0,
                location: 1,
                format: vk::Format::R32G32B32A32_SFLOAT,
                offset: 8,
            },
            vk::VertexInputAttributeDescription {
                binding: 0,
                location: 2,
                format: vk::Format::R32G32_SFLOAT,
                offset: 24,
            },
        ]
    }
}

/// A batch of overlay quads to draw this frame.
pub struct OverlayBatch {
    pub vertices: Vec<OverlayVertex>,
    pub indices: Vec<u32>,
    font: BitmapFont,
    /// Shared registry populated by `OverlayRenderer` as icon
    /// PNGs stream in during loading. Cloned `Arc` \u2014 mutations
    /// from the renderer become visible here automatically.
    icon_uv: IconUvRegistry,
}

impl OverlayBatch {
    pub fn new() -> Self {
        Self {
            vertices: Vec::new(),
            indices: Vec::new(),
            font: BitmapFont::new(),
            icon_uv: IconUvRegistry::default(),
        }
    }

    /// Bind to the renderer's shared icon UV registry and resync
    /// the batch's internal font to match the actual overlay-atlas
    /// dimensions. The atlas grows in height with the icon count;
    /// without this resync, glyph UVs would still be computed
    /// against the default size and sample into the icon region.
    /// Called once by the renderer after `OverlayRenderer::new`.
    pub fn bind_overlay_atlas(
        &mut self,
        icon_uv: IconUvRegistry,
        atlas_width: u32,
        atlas_height: u32,
    ) {
        self.icon_uv = icon_uv;
        self.font = BitmapFont::with_atlas_size(atlas_width, atlas_height);
    }

    pub fn clear(&mut self) {
        self.vertices.clear();
        self.indices.clear();
    }

    /// UV for the solid-white region of the atlas (top-left 1x1 pixel area).
    fn white_uv() -> [f32; 2] {
        // The font atlas has a solid white pixel at (0,0)
        [0.0, 0.0]
    }

    /// Add a filled rectangle. Coords in NDC (-1..1).
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) {
        let uv = Self::white_uv();
        let base = self.vertices.len() as u32;
        self.vertices.push(OverlayVertex {
            position: [x, y],
            color,
            uv,
        });
        self.vertices.push(OverlayVertex {
            position: [x + w, y],
            color,
            uv,
        });
        self.vertices.push(OverlayVertex {
            position: [x + w, y + h],
            color,
            uv,
        });
        self.vertices.push(OverlayVertex {
            position: [x, y + h],
            color,
            uv,
        });
        self.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    /// Add a rectangle with a vertical gradient: `top` colour
    /// at the upper edge, `bot` at the lower. The fragment
    /// shader is `fragColor * texture(white)` so per-vertex
    /// colours interpolate linearly across the quad — no
    /// extra pipeline needed. Coords in NDC.
    pub fn rect_grad_v(&mut self, x: f32, y: f32, w: f32, h: f32, top: [f32; 4], bot: [f32; 4]) {
        let uv = Self::white_uv();
        let base = self.vertices.len() as u32;
        // NDC y grows downward in our pipeline; vertices at
        // `y` are the visual *top*, vertices at `y + h` are
        // the visual *bottom*.
        self.vertices.push(OverlayVertex {
            position: [x, y],
            color: top,
            uv,
        });
        self.vertices.push(OverlayVertex {
            position: [x + w, y],
            color: top,
            uv,
        });
        self.vertices.push(OverlayVertex {
            position: [x + w, y + h],
            color: bot,
            uv,
        });
        self.vertices.push(OverlayVertex {
            position: [x, y + h],
            color: bot,
            uv,
        });
        self.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    /// Add a filled rect with pixel coordinates (top-left origin).
    pub fn rect_px(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: [f32; 4],
        screen_w: f32,
        screen_h: f32,
    ) {
        let ndc_x = (x / screen_w) * 2.0 - 1.0;
        let ndc_y = (y / screen_h) * 2.0 - 1.0;
        let ndc_w = (w / screen_w) * 2.0;
        let ndc_h = (h / screen_h) * 2.0;
        self.rect(ndc_x, ndc_y, ndc_w, ndc_h, color);
    }

    /// Vertical-gradient pixel-space rect. `top` is sampled
    /// along the upper edge, `bot` along the lower edge.
    /// Linear interpolation across the quad gives a smooth
    /// gradient at zero extra cost \u2014 our fragment shader
    /// already multiplies the per-vertex colour through.
    pub fn rect_px_grad_v(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        top: [f32; 4],
        bot: [f32; 4],
        screen_w: f32,
        screen_h: f32,
    ) {
        let ndc_x = (x / screen_w) * 2.0 - 1.0;
        let ndc_y = (y / screen_h) * 2.0 - 1.0;
        let ndc_w = (w / screen_w) * 2.0;
        let ndc_h = (h / screen_h) * 2.0;
        self.rect_grad_v(ndc_x, ndc_y, ndc_w, ndc_h, top, bot);
    }

    /// Filled rounded rectangle in pixel coordinates. `radius` is
    /// the corner radius in pixels (clamped to half the smaller
    /// side). Decomposed into a centre quad, four edge quads, and
    /// four corner triangle fans so the result is a real circle in
    /// pixel space (compensates for non-square viewports).
    ///
    /// For `radius <= 0.0` falls back to [`Self::rect_px`] so the
    /// caller can pass `theme.spacing.corner_radius` unconditionally.
    pub fn rounded_rect_px(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        color: [f32; 4],
        screen_w: f32,
        screen_h: f32,
    ) {
        if radius <= 0.0 || w <= 0.0 || h <= 0.0 {
            if w > 0.0 && h > 0.0 {
                self.rect_px(x, y, w, h, color, screen_w, screen_h);
            }
            return;
        }
        let r = radius.min(w * 0.5).min(h * 0.5);

        // Centre.
        self.rect_px(
            x + r,
            y + r,
            w - 2.0 * r,
            h - 2.0 * r,
            color,
            screen_w,
            screen_h,
        );
        // Top + bottom edges (between the two top/bottom corners).
        self.rect_px(x + r, y, w - 2.0 * r, r, color, screen_w, screen_h);
        self.rect_px(x + r, y + h - r, w - 2.0 * r, r, color, screen_w, screen_h);
        // Left + right edges.
        self.rect_px(x, y + r, r, h - 2.0 * r, color, screen_w, screen_h);
        self.rect_px(x + w - r, y + r, r, h - 2.0 * r, color, screen_w, screen_h);

        // Corner fans. Segment count scales with radius so small
        // radii don't pay for unnecessary triangles, but a 32 px
        // dialog corner still looks smooth.
        let segments = (r.ceil() as u32).clamp(3, 16);
        // (centre_x, centre_y, start_angle, end_angle)
        const HALF_PI: f32 = std::f32::consts::FRAC_PI_2;
        const PI: f32 = std::f32::consts::PI;
        let corners: [(f32, f32, f32); 4] = [
            (x + r, y + r, PI),           // TL: PI .. 1.5*PI
            (x + w - r, y + r, 1.5 * PI), // TR: 1.5PI .. 2PI
            (x + w - r, y + h - r, 0.0),  // BR: 0 .. PI/2
            (x + r, y + h - r, HALF_PI),  // BL: PI/2 .. PI
        ];
        for (cx, cy, start) in corners {
            self.corner_fan_px(
                cx, cy, r, start, HALF_PI, segments, color, screen_w, screen_h,
            );
        }
    }

    /// Triangle-fan helper used by [`Self::rounded_rect_px`]. Emits
    /// `segments` triangles spanning `[start, start + sweep]` (radians)
    /// around `(cx, cy)` with radius `r`.
    fn corner_fan_px(
        &mut self,
        cx: f32,
        cy: f32,
        r: f32,
        start: f32,
        sweep: f32,
        segments: u32,
        color: [f32; 4],
        screen_w: f32,
        screen_h: f32,
    ) {
        let uv = Self::white_uv();
        let to_ndc = |x: f32, y: f32| -> [f32; 2] {
            [(x / screen_w) * 2.0 - 1.0, (y / screen_h) * 2.0 - 1.0]
        };
        let centre = self.vertices.len() as u32;
        self.vertices.push(OverlayVertex {
            position: to_ndc(cx, cy),
            color,
            uv,
        });
        let step = sweep / segments as f32;
        for i in 0..=segments {
            let a = start + step * i as f32;
            let px = cx + a.cos() * r;
            let py = cy + a.sin() * r;
            self.vertices.push(OverlayVertex {
                position: to_ndc(px, py),
                color,
                uv,
            });
            if i > 0 {
                // The just-pushed arc point is at `centre + i + 1`
                // (centre at offset 0, arc[0]..arc[segments] at
                // offsets 1..=segments+1). Each triangle spans
                // `centre`, `arc[i-1]`, `arc[i]`. Off-by-one in
                // either index leaves a wedge gap at one end of
                // the arc — invisible at small radii but very
                // visible as a hairline notch / "squiggly line"
                // at the new 8 px corner radius.
                let prev = centre + i; // arc[i-1]
                let curr = centre + i + 1; // arc[i]
                self.indices.extend_from_slice(&[centre, prev, curr]);
            }
        }
    }

    /// Filled rounded rect with a vertical gradient: `top` along
    /// the upper edge of the bounding box, `bot` along the lower.
    /// Same decomposition as `rounded_rect_px` (centre + 4 edge
    /// rects + 4 corner fans), but each sub-region's vertex
    /// colours are lerped from `top`/`bot` based on their y
    /// position so the gradient is continuous across the whole
    /// shape — no visible seams between sub-rects.
    pub fn rounded_rect_px_grad_v(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        top: [f32; 4],
        bot: [f32; 4],
        screen_w: f32,
        screen_h: f32,
    ) {
        if radius <= 0.0 || w <= 0.0 || h <= 0.0 {
            if w > 0.0 && h > 0.0 {
                self.rect_px_grad_v(x, y, w, h, top, bot, screen_w, screen_h);
            }
            return;
        }
        let r = radius.min(w * 0.5).min(h * 0.5);
        let lerp = |t: f32| -> [f32; 4] {
            [
                top[0] + (bot[0] - top[0]) * t,
                top[1] + (bot[1] - top[1]) * t,
                top[2] + (bot[2] - top[2]) * t,
                top[3] + (bot[3] - top[3]) * t,
            ]
        };
        let inv_h = 1.0 / h;
        let c_top_inner = lerp(r * inv_h);
        let c_bot_inner = lerp((h - r) * inv_h);
        // Top edge band.
        self.rect_px_grad_v(
            x + r,
            y,
            w - 2.0 * r,
            r,
            top,
            c_top_inner,
            screen_w,
            screen_h,
        );
        // Centre.
        self.rect_px_grad_v(
            x + r,
            y + r,
            w - 2.0 * r,
            h - 2.0 * r,
            c_top_inner,
            c_bot_inner,
            screen_w,
            screen_h,
        );
        // Bottom edge band.
        self.rect_px_grad_v(
            x + r,
            y + h - r,
            w - 2.0 * r,
            r,
            c_bot_inner,
            bot,
            screen_w,
            screen_h,
        );
        // Side bands.
        self.rect_px_grad_v(
            x,
            y + r,
            r,
            h - 2.0 * r,
            c_top_inner,
            c_bot_inner,
            screen_w,
            screen_h,
        );
        self.rect_px_grad_v(
            x + w - r,
            y + r,
            r,
            h - 2.0 * r,
            c_top_inner,
            c_bot_inner,
            screen_w,
            screen_h,
        );
        // Corner fans.
        let segments = (r.ceil() as u32).clamp(3, 16);
        const HALF_PI: f32 = std::f32::consts::FRAC_PI_2;
        const PI: f32 = std::f32::consts::PI;
        let corners: [(f32, f32, f32); 4] = [
            (x + r, y + r, PI),
            (x + w - r, y + r, 1.5 * PI),
            (x + w - r, y + h - r, 0.0),
            (x + r, y + h - r, HALF_PI),
        ];
        for (cx, cy, start) in corners {
            self.corner_fan_px_grad_v(
                cx, cy, r, start, HALF_PI, segments, y, inv_h, top, bot, screen_w, screen_h,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn corner_fan_px_grad_v(
        &mut self,
        cx: f32,
        cy: f32,
        r: f32,
        start: f32,
        sweep: f32,
        segments: u32,
        y0: f32,
        inv_h: f32,
        top: [f32; 4],
        bot: [f32; 4],
        screen_w: f32,
        screen_h: f32,
    ) {
        let uv = Self::white_uv();
        let to_ndc = |x: f32, y: f32| -> [f32; 2] {
            [(x / screen_w) * 2.0 - 1.0, (y / screen_h) * 2.0 - 1.0]
        };
        let lerp_color = |vy: f32| -> [f32; 4] {
            let t = ((vy - y0) * inv_h).clamp(0.0, 1.0);
            [
                top[0] + (bot[0] - top[0]) * t,
                top[1] + (bot[1] - top[1]) * t,
                top[2] + (bot[2] - top[2]) * t,
                top[3] + (bot[3] - top[3]) * t,
            ]
        };
        let centre = self.vertices.len() as u32;
        self.vertices.push(OverlayVertex {
            position: to_ndc(cx, cy),
            color: lerp_color(cy),
            uv,
        });
        let step = sweep / segments as f32;
        for i in 0..=segments {
            let a = start + step * i as f32;
            let px = cx + a.cos() * r;
            let py = cy + a.sin() * r;
            self.vertices.push(OverlayVertex {
                position: to_ndc(px, py),
                color: lerp_color(py),
                uv,
            });
            if i > 0 {
                let prev = centre + i;
                let curr = centre + i + 1;
                self.indices.extend_from_slice(&[centre, prev, curr]);
            }
        }
    }

    /// Filled rounded rect whose vertex colours are computed
    /// by a per-vertex callback. Lets callers express any
    /// gradient (radial, elliptical, axial, multi-stop …)
    /// without adding another shader pipeline. The callback
    /// is invoked once per generated vertex with that
    /// vertex's pixel-space `(x, y)` and must return the
    /// `[r, g, b, a]` colour for that point.
    ///
    /// Topology matches `rounded_rect_px` exactly (centre +
    /// 4 edge bands + 4 corner fans), so the gradient
    /// resolution is bounded by the corner-segment count
    /// (~16 vertices per corner for the typical 8-px radius).
    /// That's enough fidelity for the smooth top-down
    /// lighting effects we use here while costing the same
    /// vertex budget as the solid version.
    pub fn rounded_rect_px_color_fn<F: FnMut(f32, f32) -> [f32; 4]>(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        mut color_at: F,
        screen_w: f32,
        screen_h: f32,
    ) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let uv = Self::white_uv();
        let to_ndc = |x: f32, y: f32| -> [f32; 2] {
            [(x / screen_w) * 2.0 - 1.0, (y / screen_h) * 2.0 - 1.0]
        };
        let r = if radius <= 0.0 {
            0.0
        } else {
            radius.min(w * 0.5).min(h * 0.5)
        };
        // Helper: emit a quad whose 4 corners get their colour
        // from the callback. The four corners are passed in the
        // order TL, TR, BR, BL.
        let mut quad = |verts: &mut Vec<OverlayVertex>,
                        indices: &mut Vec<u32>,
                        p0: (f32, f32),
                        p1: (f32, f32),
                        p2: (f32, f32),
                        p3: (f32, f32),
                        color_at: &mut F| {
            let base = verts.len() as u32;
            verts.push(OverlayVertex {
                position: to_ndc(p0.0, p0.1),
                color: color_at(p0.0, p0.1),
                uv,
            });
            verts.push(OverlayVertex {
                position: to_ndc(p1.0, p1.1),
                color: color_at(p1.0, p1.1),
                uv,
            });
            verts.push(OverlayVertex {
                position: to_ndc(p2.0, p2.1),
                color: color_at(p2.0, p2.1),
                uv,
            });
            verts.push(OverlayVertex {
                position: to_ndc(p3.0, p3.1),
                color: color_at(p3.0, p3.1),
                uv,
            });
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        };
        if r <= 0.0 {
            // Single quad fast-path.
            quad(
                &mut self.vertices,
                &mut self.indices,
                (x, y),
                (x + w, y),
                (x + w, y + h),
                (x, y + h),
                &mut color_at,
            );
            return;
        }
        // Centre quad.
        quad(
            &mut self.vertices,
            &mut self.indices,
            (x + r, y + r),
            (x + w - r, y + r),
            (x + w - r, y + h - r),
            (x + r, y + h - r),
            &mut color_at,
        );
        // Top band.
        quad(
            &mut self.vertices,
            &mut self.indices,
            (x + r, y),
            (x + w - r, y),
            (x + w - r, y + r),
            (x + r, y + r),
            &mut color_at,
        );
        // Bottom band.
        quad(
            &mut self.vertices,
            &mut self.indices,
            (x + r, y + h - r),
            (x + w - r, y + h - r),
            (x + w - r, y + h),
            (x + r, y + h),
            &mut color_at,
        );
        // Left band.
        quad(
            &mut self.vertices,
            &mut self.indices,
            (x, y + r),
            (x + r, y + r),
            (x + r, y + h - r),
            (x, y + h - r),
            &mut color_at,
        );
        // Right band.
        quad(
            &mut self.vertices,
            &mut self.indices,
            (x + w - r, y + r),
            (x + w, y + r),
            (x + w, y + h - r),
            (x + w - r, y + h - r),
            &mut color_at,
        );
        // Corner fans.
        let segments = (r.ceil() as u32).clamp(4, 16);
        const HALF_PI: f32 = std::f32::consts::FRAC_PI_2;
        const PI: f32 = std::f32::consts::PI;
        let corners: [(f32, f32, f32); 4] = [
            (x + r, y + r, PI),
            (x + w - r, y + r, 1.5 * PI),
            (x + w - r, y + h - r, 0.0),
            (x + r, y + h - r, HALF_PI),
        ];
        for (cx, cy, start) in corners {
            let centre = self.vertices.len() as u32;
            self.vertices.push(OverlayVertex {
                position: to_ndc(cx, cy),
                color: color_at(cx, cy),
                uv,
            });
            let step = HALF_PI / segments as f32;
            for i in 0..=segments {
                let a = start + step * i as f32;
                let px = cx + a.cos() * r;
                let py = cy + a.sin() * r;
                self.vertices.push(OverlayVertex {
                    position: to_ndc(px, py),
                    color: color_at(px, py),
                    uv,
                });
                if i > 0 {
                    let prev = centre + i;
                    let curr = centre + i + 1;
                    self.indices.extend_from_slice(&[centre, prev, curr]);
                }
            }
        }
    }

    /// Filled rect with one colour per corner: `tl`, `tr`,
    /// `bl`, `br` (top-left, top-right, bottom-left, bottom-
    /// right). The fragment shader interpolates linearly so
    /// you get a true bilinear gradient — useful for bevel
    /// bands that fade out toward the horizontal edges.
    #[allow(clippy::too_many_arguments)]
    pub fn rect_px_grad4(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        tl: [f32; 4],
        tr: [f32; 4],
        bl: [f32; 4],
        br: [f32; 4],
        screen_w: f32,
        screen_h: f32,
    ) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let uv = Self::white_uv();
        let to_ndc = |x: f32, y: f32| -> [f32; 2] {
            [(x / screen_w) * 2.0 - 1.0, (y / screen_h) * 2.0 - 1.0]
        };
        let base = self.vertices.len() as u32;
        self.vertices.push(OverlayVertex {
            position: to_ndc(x, y),
            color: tl,
            uv,
        });
        self.vertices.push(OverlayVertex {
            position: to_ndc(x + w, y),
            color: tr,
            uv,
        });
        self.vertices.push(OverlayVertex {
            position: to_ndc(x + w, y + h),
            color: br,
            uv,
        });
        self.vertices.push(OverlayVertex {
            position: to_ndc(x, y + h),
            color: bl,
            uv,
        });
        self.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    /// Filled rounded rect with an elliptical radial gradient:
    /// `centre` colour at the geometric centre, lerping out to
    /// `edge` along the bounding ellipse. Smoothstep falloff so
    /// the highlight reads as a soft oval (forge-iron / blood-
    /// red button) rather than a hard hotspot.
    #[allow(clippy::too_many_arguments)]
    pub fn rounded_rect_px_radial(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        edge: [f32; 4],
        centre: [f32; 4],
        screen_w: f32,
        screen_h: f32,
    ) {
        let cx = x + w * 0.5;
        let cy = y + h * 0.5;
        // Half-axes of the elliptical hotspot. Deliberately
        // *narrower than* half-width and *much shorter than*
        // half-height so the iso-colour ellipse is fully
        // inscribed inside the rounded rect — i.e. every
        // point on the perimeter (top edge, bottom edge,
        // rounded corners) evaluates to `edge`. That kills
        // the "solid wedge at the corners" artefact you'd
        // get if the hotspot reached the rounded corner
        // fans (the fan centres would then sample a non-
        // edge colour and the few-segment corners would
        // read as a solid mid-tone region distinct from
        // the smooth gradient on the long edges).
        //
        // The shape reads as a horizontal oval: ax > ay.
        let ax = (w * 0.50).max(1.0);
        let ay = (h * 0.30).max(1.0);
        let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
        self.rounded_rect_px_color_fn(
            x,
            y,
            w,
            h,
            radius,
            |px, py| {
                let dx = (px - cx) / ax;
                let dy = (py - cy) / ay;
                let d = (dx * dx + dy * dy).sqrt().min(1.0);
                // smoothstep(1, 0, d): 1 at centre, 0 at edge.
                let t = 1.0 - d;
                let s = t * t * (3.0 - 2.0 * t);
                [
                    lerp(edge[0], centre[0], s),
                    lerp(edge[1], centre[1], s),
                    lerp(edge[2], centre[2], s),
                    lerp(edge[3], centre[3], s),
                ]
            },
            screen_w,
            screen_h,
        );
    }

    /// True rounded-rect outline of constant pixel thickness.
    /// Builds a closed triangle strip around the perimeter so
    /// the outline wraps the rounded corners cleanly — unlike
    /// the four-edge-rect approximation in [`Ui::draw_rounded_outline`],
    /// this draws the curved corner runs explicitly. Use this
    /// for inset hairlines that have nothing behind them to
    /// hide the corner gaps.
    #[allow(clippy::too_many_arguments)]
    pub fn rounded_outline_px(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        thickness: f32,
        color: [f32; 4],
        screen_w: f32,
        screen_h: f32,
    ) {
        if thickness <= 0.0 || w <= 0.0 || h <= 0.0 {
            return;
        }
        let r = radius.min(w * 0.5).min(h * 0.5).max(0.0);
        let t = thickness.min(r.max(thickness));
        let uv = Self::white_uv();
        let to_ndc = |x: f32, y: f32| -> [f32; 2] {
            [(x / screen_w) * 2.0 - 1.0, (y / screen_h) * 2.0 - 1.0]
        };
        // Generate the perimeter samples: 4 straight runs + 4
        // corner arcs. For each sample we emit two vertices
        // (outer + inner), then connect consecutive samples
        // with two triangles.
        const HALF_PI: f32 = std::f32::consts::FRAC_PI_2;
        const PI: f32 = std::f32::consts::PI;
        let segments = (r.ceil() as u32).clamp(4, 16);
        // Corner centre + start angle. Order matches a
        // clockwise sweep starting at the top-left corner so
        // the strip closes correctly.
        let corners: [(f32, f32, f32); 4] = [
            (x + r, y + r, PI),           // top-left
            (x + w - r, y + r, 1.5 * PI), // top-right
            (x + w - r, y + h - r, 0.0),  // bottom-right
            (x + r, y + h - r, HALF_PI),  // bottom-left
        ];
        // Collect (outer, inner) sample pairs.
        let mut samples: Vec<((f32, f32), (f32, f32))> = Vec::with_capacity(64);
        for i in 0..4 {
            // Straight edge run from previous corner end to
            // this corner start. The outer/inner endpoints
            // sit on the rect boundary tangents, no
            // interpolation needed (just two endpoints).
            let (cx, cy, start) = corners[i];
            // Start point of this corner's arc:
            let sx = cx + start.cos() * r;
            let sy = cy + start.sin() * r;
            let inv_normal = ((sx - cx) * (sx - cx) + (sy - cy) * (sy - cy))
                .sqrt()
                .max(1e-6);
            let nx = (sx - cx) / inv_normal;
            let ny = (sy - cy) / inv_normal;
            samples.push(((sx, sy), (sx - nx * t, sy - ny * t)));
            // Walk the arc.
            let step = HALF_PI / segments as f32;
            for s in 1..=segments {
                let a = start + step * s as f32;
                let ox = cx + a.cos() * r;
                let oy = cy + a.sin() * r;
                let dx = a.cos();
                let dy = a.sin();
                samples.push(((ox, oy), (ox - dx * t, oy - dy * t)));
            }
        }
        // Emit triangles between consecutive samples, closing
        // the strip back to sample 0.
        let n = samples.len();
        let base = self.vertices.len() as u32;
        for ((ox, oy), (ix, iy)) in &samples {
            self.vertices.push(OverlayVertex {
                position: to_ndc(*ox, *oy),
                color,
                uv,
            });
            self.vertices.push(OverlayVertex {
                position: to_ndc(*ix, *iy),
                color,
                uv,
            });
        }
        for i in 0..n {
            let j = (i + 1) % n;
            let o0 = base + (i as u32) * 2;
            let i0 = base + (i as u32) * 2 + 1;
            let o1 = base + (j as u32) * 2;
            let i1 = base + (j as u32) * 2 + 1;
            self.indices.extend_from_slice(&[o0, o1, i1, o0, i1, i0]);
        }
    }

    /// Filled rounded rect with an elliptical radial gradient
    /// AND a procedural cloud-noise modulation applied per
    /// fragment in the shader. Same primitive cost as
    /// [`Self::rounded_rect_px_radial`] — we just emit a
    /// sentinel UV (`-1, -1`) so the shader knows to apply
    /// noise. Use for any panel/button surface that should
    /// read as textured stone or hammered metal rather than
    /// flat gradient.
    #[allow(clippy::too_many_arguments)]
    pub fn rounded_rect_px_radial_noisy(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        edge: [f32; 4],
        centre: [f32; 4],
        screen_w: f32,
        screen_h: f32,
    ) {
        let cx = x + w * 0.5;
        let cy = y + h * 0.5;
        let ax = (w * 0.50).max(1.0);
        let ay = (h * 0.30).max(1.0);
        let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
        // Replace UV with the noise sentinel so the shader
        // turns on its noise path. We can't reuse
        // `rounded_rect_px_color_fn` directly because that
        // emits white-pixel UVs — so we duplicate the
        // tessellation here with the sentinel UV inline.
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let to_ndc = |x: f32, y: f32| -> [f32; 2] {
            [(x / screen_w) * 2.0 - 1.0, (y / screen_h) * 2.0 - 1.0]
        };
        let r = radius.min(w * 0.5).min(h * 0.5).max(0.0);
        let color_at = |px: f32, py: f32| -> [f32; 4] {
            let dx = (px - cx) / ax;
            let dy = (py - cy) / ay;
            let d = (dx * dx + dy * dy).sqrt().min(1.0);
            let tn = 1.0 - d;
            let s = tn * tn * (3.0 - 2.0 * tn);
            [
                lerp(edge[0], centre[0], s),
                lerp(edge[1], centre[1], s),
                lerp(edge[2], centre[2], s),
                lerp(edge[3], centre[3], s),
            ]
        };
        // Sentinel UV — fragment shader treats UV.x < 0 as
        // "apply procedural noise + sample white".
        const NOISE_UV: [f32; 2] = [-1.0, -1.0];
        let mut quad = |verts: &mut Vec<OverlayVertex>,
                        indices: &mut Vec<u32>,
                        p0: (f32, f32),
                        p1: (f32, f32),
                        p2: (f32, f32),
                        p3: (f32, f32)| {
            let base = verts.len() as u32;
            verts.push(OverlayVertex {
                position: to_ndc(p0.0, p0.1),
                color: color_at(p0.0, p0.1),
                uv: NOISE_UV,
            });
            verts.push(OverlayVertex {
                position: to_ndc(p1.0, p1.1),
                color: color_at(p1.0, p1.1),
                uv: NOISE_UV,
            });
            verts.push(OverlayVertex {
                position: to_ndc(p2.0, p2.1),
                color: color_at(p2.0, p2.1),
                uv: NOISE_UV,
            });
            verts.push(OverlayVertex {
                position: to_ndc(p3.0, p3.1),
                color: color_at(p3.0, p3.1),
                uv: NOISE_UV,
            });
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        };
        if r <= 0.0 {
            quad(
                &mut self.vertices,
                &mut self.indices,
                (x, y),
                (x + w, y),
                (x + w, y + h),
                (x, y + h),
            );
            return;
        }
        // Centre + 4 edge bands.
        quad(
            &mut self.vertices,
            &mut self.indices,
            (x + r, y + r),
            (x + w - r, y + r),
            (x + w - r, y + h - r),
            (x + r, y + h - r),
        );
        quad(
            &mut self.vertices,
            &mut self.indices,
            (x + r, y),
            (x + w - r, y),
            (x + w - r, y + r),
            (x + r, y + r),
        );
        quad(
            &mut self.vertices,
            &mut self.indices,
            (x + r, y + h - r),
            (x + w - r, y + h - r),
            (x + w - r, y + h),
            (x + r, y + h),
        );
        quad(
            &mut self.vertices,
            &mut self.indices,
            (x, y + r),
            (x + r, y + r),
            (x + r, y + h - r),
            (x, y + h - r),
        );
        quad(
            &mut self.vertices,
            &mut self.indices,
            (x + w - r, y + r),
            (x + w, y + r),
            (x + w, y + h - r),
            (x + w - r, y + h - r),
        );
        // Corner fans.
        let segments = (r.ceil() as u32).clamp(4, 16);
        const HALF_PI: f32 = std::f32::consts::FRAC_PI_2;
        const PI: f32 = std::f32::consts::PI;
        let corners: [(f32, f32, f32); 4] = [
            (x + r, y + r, PI),
            (x + w - r, y + r, 1.5 * PI),
            (x + w - r, y + h - r, 0.0),
            (x + r, y + h - r, HALF_PI),
        ];
        for (cx, cy, start) in corners {
            let centre_idx = self.vertices.len() as u32;
            self.vertices.push(OverlayVertex {
                position: to_ndc(cx, cy),
                color: color_at(cx, cy),
                uv: NOISE_UV,
            });
            let step = HALF_PI / segments as f32;
            for i in 0..=segments {
                let a = start + step * i as f32;
                let px = cx + a.cos() * r;
                let py = cy + a.sin() * r;
                self.vertices.push(OverlayVertex {
                    position: to_ndc(px, py),
                    color: color_at(px, py),
                    uv: NOISE_UV,
                });
                if i > 0 {
                    let prev = centre_idx + i;
                    let curr = centre_idx + i + 1;
                    self.indices.extend_from_slice(&[centre_idx, prev, curr]);
                }
            }
        }
    }

    /// Filled line segment from `(x0, y0)` to `(x1, y1)` with
    /// the given pixel `thickness`. Emitted as a single
    /// rotated quad — the two long edges run parallel to the
    /// segment, the two short ends are perpendicular caps.
    /// No end-caps beyond the perpendicular butt, no antialias
    /// — fine for diagrammatic edges (graph viewers, talent
    /// trees, debug overlays). Degenerate inputs (zero length
    /// or non-positive thickness) are dropped silently.
    pub fn line_px(
        &mut self,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        thickness: f32,
        color: [f32; 4],
        screen_w: f32,
        screen_h: f32,
    ) {
        if thickness <= 0.0 {
            return;
        }
        let dx = x1 - x0;
        let dy = y1 - y0;
        let len = (dx * dx + dy * dy).sqrt();
        if len <= 1e-6 {
            return;
        }
        // Perpendicular unit vector × half-thickness — the
        // four corners are then p0 ± n and p1 ± n.
        let nx = -dy / len * thickness * 0.5;
        let ny = dx / len * thickness * 0.5;
        let uv = Self::white_uv();
        let to_ndc = |x: f32, y: f32| -> [f32; 2] {
            [(x / screen_w) * 2.0 - 1.0, (y / screen_h) * 2.0 - 1.0]
        };
        let base = self.vertices.len() as u32;
        self.vertices.push(OverlayVertex {
            position: to_ndc(x0 + nx, y0 + ny),
            color,
            uv,
        });
        self.vertices.push(OverlayVertex {
            position: to_ndc(x1 + nx, y1 + ny),
            color,
            uv,
        });
        self.vertices.push(OverlayVertex {
            position: to_ndc(x1 - nx, y1 - ny),
            color,
            uv,
        });
        self.vertices.push(OverlayVertex {
            position: to_ndc(x0 - nx, y0 - ny),
            color,
            uv,
        });
        self.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    /// Bevelled stone disc with an indented dark-to-tint radial
    /// gradient and a shader-rasterised glow halo. One quad,
    /// the fragment shader does everything via SDF math + the
    /// `[-2.0, -1.0]` UV sentinel.
    ///
    /// * `radius` — solid-disc radius in pixels.
    /// * `halo` — extra pixels of glow halo to leave around
    ///   the disc. The emitted quad is sized
    ///   `2 * (radius + halo)` so the halo has room to fade
    ///   into the background.
    /// * `color` — node tint. Alpha drives the master
    ///   brightness (the gating-driven dimming curve).
    pub fn glow_disc(
        &mut self,
        cx: f32,
        cy: f32,
        radius: f32,
        halo: f32,
        color: [f32; 4],
        screen_w: f32,
        screen_h: f32,
    ) {
        if radius <= 0.0 {
            return;
        }
        let halo = halo.max(0.0);
        // The shader hard-codes `CORE_R = 0.65` as the local-r
        // boundary between solid disc and halo. We size the
        // quad so that `radius` lands exactly at that boundary:
        //     quad_half = radius / CORE_R
        // and the halo extent is the remainder
        //     halo_extent = quad_half - radius = radius * (1/0.65 - 1)
        //                ≈ radius * 0.538
        // The caller's `halo` parameter is taken as a *minimum*
        // — we expand the quad if needed but never shrink the
        // shader-fixed halo band, otherwise the visual halo
        // gets clipped against the quad edge.
        let core_r: f32 = 0.65;
        let quad_half = (radius / core_r).max(radius + halo);
        let x = cx - quad_half;
        let y = cy - quad_half;
        let w = quad_half * 2.0;
        let h = quad_half * 2.0;
        let to_ndc = |px: f32, py: f32| -> [f32; 2] {
            [(px / screen_w) * 2.0 - 1.0, (py / screen_h) * 2.0 - 1.0]
        };
        // UV in [-2.0, -1.1] per axis — the shader maps
        // back to local [0, 1] for the SDF. The 0.1 guard
        // band keeps interpolated corner pixels well clear
        // of the -1.0 atlas-sampling sentinel boundary.
        const U0: f32 = -2.0;
        const U1: f32 = -1.1;
        let base = self.vertices.len() as u32;
        self.vertices.push(OverlayVertex {
            position: to_ndc(x, y),
            color,
            uv: [U0, U0],
        });
        self.vertices.push(OverlayVertex {
            position: to_ndc(x + w, y),
            color,
            uv: [U1, U0],
        });
        self.vertices.push(OverlayVertex {
            position: to_ndc(x + w, y + h),
            color,
            uv: [U1, U1],
        });
        self.vertices.push(OverlayVertex {
            position: to_ndc(x, y + h),
            color,
            uv: [U0, U1],
        });
        self.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    /// Glowing line from `(x0, y0)` to `(x1, y1)`. Emitted as
    /// a rotated quad wider than the visible line — the shader
    /// rasterises an exponential glow falloff across the
    /// perpendicular axis using the `[-3.0, -2.0]` UV sentinel.
    ///
    /// * `core_thickness` — width of the bright core in pixels.
    /// * `halo_extent` — extra pixels each side that the halo
    ///   fades over. Total quad width = `core + 2 * halo`.
    pub fn glow_line(
        &mut self,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        core_thickness: f32,
        halo_extent: f32,
        color: [f32; 4],
        screen_w: f32,
        screen_h: f32,
    ) {
        if core_thickness <= 0.0 {
            return;
        }
        let dx = x1 - x0;
        let dy = y1 - y0;
        let len = (dx * dx + dy * dy).sqrt();
        if len <= 1e-6 {
            return;
        }
        let total = core_thickness + halo_extent.max(0.0) * 2.0;
        let nx = -dy / len * total * 0.5;
        let ny = dx / len * total * 0.5;
        let to_ndc = |x: f32, y: f32| -> [f32; 2] {
            [(x / screen_w) * 2.0 - 1.0, (y / screen_h) * 2.0 - 1.0]
        };
        // UV.x in [-3.0, -2.1] maps to *across-axis* in
        // [0, 1] (0.5 = line core). 0.1 guard band keeps
        // interpolated pixels clear of the -2.0 sentinel.
        const A0: f32 = -3.0;
        const A1: f32 = -2.1;
        let base = self.vertices.len() as u32;
        self.vertices.push(OverlayVertex {
            position: to_ndc(x0 + nx, y0 + ny),
            color,
            uv: [A0, A0],
        });
        self.vertices.push(OverlayVertex {
            position: to_ndc(x1 + nx, y1 + ny),
            color,
            uv: [A0, A1],
        });
        self.vertices.push(OverlayVertex {
            position: to_ndc(x1 - nx, y1 - ny),
            color,
            uv: [A1, A1],
        });
        self.vertices.push(OverlayVertex {
            position: to_ndc(x0 - nx, y0 - ny),
            color,
            uv: [A1, A0],
        });
        self.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    /// Draw a text string at pixel position (top-left origin).
    /// Returns the width in pixels of the rendered text.
    pub fn text(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        size: f32,
        color: [f32; 4],
        screen_w: f32,
        screen_h: f32,
    ) -> f32 {
        // Scale from rasterised pixels to display pixels.
        let scale = size / self.font.raster_px;
        let mut cursor_x = x;

        for ch in text.chars() {
            let Some(glyph) = self.font.glyph(ch) else {
                // Unknown char — still advance by a half-em
                // so cursor positioning matches `measure_text`.
                cursor_x += self.font.advance(ch) * scale;
                continue;
            };
            // Empty bitmap (e.g. space): advance only.
            if glyph.w_px <= 0.0 || glyph.h_px <= 0.0 {
                cursor_x += glyph.advance * scale;
                continue;
            }

            let gx = cursor_x + glyph.x_offset * scale;
            let gy = y + glyph.y_offset * scale;
            let gw = glyph.w_px * scale;
            let gh = glyph.h_px * scale;

            // Convert pixel position to NDC.
            let ndc_x = (gx / screen_w) * 2.0 - 1.0;
            let ndc_y = (gy / screen_h) * 2.0 - 1.0;
            let ndc_w = (gw / screen_w) * 2.0;
            let ndc_h = (gh / screen_h) * 2.0;

            let base = self.vertices.len() as u32;
            self.vertices.push(OverlayVertex {
                position: [ndc_x, ndc_y],
                color,
                uv: [glyph.u0, glyph.v0],
            });
            self.vertices.push(OverlayVertex {
                position: [ndc_x + ndc_w, ndc_y],
                color,
                uv: [glyph.u1, glyph.v0],
            });
            self.vertices.push(OverlayVertex {
                position: [ndc_x + ndc_w, ndc_y + ndc_h],
                color,
                uv: [glyph.u1, glyph.v1],
            });
            self.vertices.push(OverlayVertex {
                position: [ndc_x, ndc_y + ndc_h],
                color,
                uv: [glyph.u0, glyph.v1],
            });
            self.indices
                .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);

            cursor_x += glyph.advance * scale;
        }

        cursor_x - x
    }

    /// Measure text width in pixels without drawing.
    pub fn measure_text(&self, text: &str, size: f32) -> f32 {
        let scale = size / self.font.raster_px;
        text.chars().map(|c| self.font.advance(c)).sum::<f32>() * scale
    }

    /// Draw a registered icon at pixel position (top-left origin).
    /// `name` matches the key the renderer registered the icon
    /// under (typically the PNG filename without extension, e.g.
    /// "Hunter_3"). `tint` is multiplied with the icon's RGBA \u2014
    /// pass `[1.0, 1.0, 1.0, 1.0]` to keep the original colours.
    /// Silently no-ops on an unknown name so callers can fall
    /// back to a placeholder rect / glyph without branching.
    pub fn icon(
        &mut self,
        name: &str,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        tint: [f32; 4],
        screen_w: f32,
        screen_h: f32,
    ) -> bool {
        let Some(&[u0, v0, u1, v1]) = self.icon_uv.read().unwrap().get(name) else {
            return false;
        };
        let ndc_x = (x / screen_w) * 2.0 - 1.0;
        let ndc_y = (y / screen_h) * 2.0 - 1.0;
        let ndc_w = (w / screen_w) * 2.0;
        let ndc_h = (h / screen_h) * 2.0;
        let base = self.vertices.len() as u32;
        self.vertices.push(OverlayVertex {
            position: [ndc_x, ndc_y],
            color: tint,
            uv: [u0, v0],
        });
        self.vertices.push(OverlayVertex {
            position: [ndc_x + ndc_w, ndc_y],
            color: tint,
            uv: [u1, v0],
        });
        self.vertices.push(OverlayVertex {
            position: [ndc_x + ndc_w, ndc_y + ndc_h],
            color: tint,
            uv: [u1, v1],
        });
        self.vertices.push(OverlayVertex {
            position: [ndc_x, ndc_y + ndc_h],
            color: tint,
            uv: [u0, v1],
        });
        self.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        true
    }

    /// True when an icon was registered under `name`.
    pub fn has_icon(&self, name: &str) -> bool {
        self.icon_uv.read().unwrap().contains_key(name)
    }

    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
}

/// Manages the overlay pipeline, font texture, and per-frame GPU buffers.
pub struct OverlayRenderer {
    pub pipeline: vk::Pipeline,
    pub pipeline_layout: vk::PipelineLayout,
    descriptor_set_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    descriptor_set: vk::DescriptorSet,
    font_image: vk::Image,
    font_image_view: vk::ImageView,
    font_sampler: vk::Sampler,
    font_allocation: Option<gpu_allocator::vulkan::Allocation>,
    vertex_buffers: Vec<Option<GpuBuffer>>,
    index_buffers: Vec<Option<GpuBuffer>>,
    index_counts: Vec<u32>,
    /// Shared registry of `name -> uv-rect` for icons painted
    /// into the atlas. Populated incrementally by
    /// [`Self::step_load_icons`]. Hand the clone to
    /// `OverlayBatch::bind_overlay_atlas` once at startup; new
    /// entries are visible automatically thereafter.
    icon_uv: IconUvRegistry,
    /// Final dimensions of the composited overlay atlas. Width is
    /// fixed but height grows with the icon count, so consumers
    /// (e.g. `OverlayBatch`'s glyph UVs) need to be resynced.
    atlas_width: u32,
    atlas_height: u32,
    /// Icon PNGs discovered at startup, paired with their
    /// registry key (the path stem relative to `assets/icons/`,
    /// using forward slashes — e.g. `loot/Boots/Boots_1` or
    /// `Hunter_3`). Consumed in order by [`Self::step_load_icons`]
    /// across many frames so the loading screen stays responsive.
    /// Indexed via [`Self::next_icon_idx`] rather than popped
    /// from the front — popping a `Vec`'s head is O(n).
    pending_icon_paths: Vec<(std::path::PathBuf, String)>,
    /// Cursor into [`Self::pending_icon_paths`] of the next
    /// icon to decode. When `next_icon_idx >= pending_icon_paths.len()`
    /// streaming is complete.
    next_icon_idx: usize,
    /// Total icons discovered (for progress reporting).
    total_icons: usize,
    /// Icons whose decode + upload has completed (or been
    /// permanently skipped). Used as the slot index for the
    /// next icon and as the "loaded" half of progress.
    loaded_icons: usize,
}

impl OverlayRenderer {
    pub fn new(
        device: &ash::Device,
        allocator: &std::sync::Arc<std::sync::Mutex<gpu_allocator::vulkan::Allocator>>,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        render_pass: vk::RenderPass,
        extent: vk::Extent2D,
        shader_dir: &std::path::Path,
    ) -> Result<Self> {
        // Build the overlay atlas in two stages:
        //   1. Synchronously paint the font glyphs (top-left
        //      97x48 region) so the loading screen \u2014 which
        //      starts drawing the very next frame \u2014 has working
        //      glyph UVs.
        //   2. Stream icon PNGs in across many frames via
        //      [`Self::step_load_icons`] so decode + resampling
        //      work doesn't stall the window before the loading
        //      screen appears.
        //
        // The atlas image is sized up-front to fit every PNG in
        // `assets/icons/`; the icon region starts as zeros and
        // is filled in via sub-region uploads as icons load.
        let icon_paths = discover_icon_paths();
        let total_icons = icon_paths.len();
        let atlas_w = crate::renderer::font::OVERLAY_ATLAS_SIZE;
        let atlas_h = compute_atlas_height(total_icons as u32);

        let font = BitmapFont::with_atlas_size(atlas_w, atlas_h);
        let atlas_data = font.atlas_data();

        let (font_image, font_allocation) = Self::create_font_image(
            device,
            allocator,
            queue,
            command_pool,
            &atlas_data,
            atlas_w,
            atlas_h,
        )?;

        let font_image_view = Self::create_image_view(device, font_image)?;
        let font_sampler = Self::create_sampler(device)?;

        // Descriptor set for the font texture
        let descriptor_set_layout = Self::create_descriptor_set_layout(device)?;
        let descriptor_pool = Self::create_descriptor_pool(device)?;
        let descriptor_set =
            Self::allocate_descriptor_set(device, descriptor_pool, descriptor_set_layout)?;
        Self::update_descriptor_set(device, descriptor_set, font_image_view, font_sampler);

        let (pipeline, pipeline_layout) = Self::create_pipeline(
            device,
            render_pass,
            extent,
            descriptor_set_layout,
            shader_dir,
        )?;

        log::info!(
            "overlay: atlas {}x{}, {} icon(s) queued for streaming load",
            atlas_w,
            atlas_h,
            total_icons,
        );

        Ok(Self {
            pipeline,
            pipeline_layout,
            descriptor_set_layout,
            descriptor_pool,
            descriptor_set,
            font_image,
            font_image_view,
            font_sampler,
            font_allocation: Some(font_allocation),
            vertex_buffers: (0..MAX_FRAMES_IN_FLIGHT).map(|_| None).collect(),
            index_buffers: (0..MAX_FRAMES_IN_FLIGHT).map(|_| None).collect(),
            index_counts: vec![0; MAX_FRAMES_IN_FLIGHT],
            icon_uv: IconUvRegistry::default(),
            atlas_width: atlas_w,
            atlas_height: atlas_h,
            pending_icon_paths: icon_paths,
            next_icon_idx: 0,
            total_icons,
            loaded_icons: 0,
        })
    }

    /// Final dimensions of the composited overlay atlas. The
    /// height grows with the icon count, so callers that compute
    /// UVs in pixel space need this to normalize correctly.
    pub fn atlas_size(&self) -> (u32, u32) {
        (self.atlas_width, self.atlas_height)
    }

    /// Cloneable handle to the shared icon UV registry. Pass to
    /// `OverlayBatch::bind_overlay_atlas` once at startup; later
    /// `step_load_icons` calls update it in place.
    pub fn icon_uv_registry(&self) -> IconUvRegistry {
        self.icon_uv.clone()
    }

    /// Total icons discovered at startup (for progress UI).
    pub fn total_icons(&self) -> usize {
        self.total_icons
    }
    /// Icons whose decode + upload has completed.
    pub fn loaded_icons(&self) -> usize {
        self.loaded_icons
    }

    /// Decode + upload up to `budget` queued icon PNGs into the
    /// atlas's icon region, registering each one's UV rect.
    /// All icons in this call are batched into a single staging
    /// buffer and a single command-buffer submit, which is far
    /// cheaper than one submit per icon (the previous approach
    /// idled the GPU on a fence wait between every 48×48 copy).
    /// Returns `(loaded, total)` for progress reporting; loading
    /// is complete when `loaded == total`.
    pub fn step_load_icons(
        &mut self,
        device: &ash::Device,
        allocator: &std::sync::Arc<std::sync::Mutex<gpu_allocator::vulkan::Allocator>>,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        budget: usize,
    ) -> Result<(usize, usize)> {
        use crate::renderer::font::ICON_REGION_Y;
        use image::imageops::FilterType;

        // (slot, name, pixels) for each icon that decoded
        // successfully and fits in the atlas. Built up in CPU
        // memory first, then committed to the GPU in a single
        // submit at the end of the call.
        let mut decoded: Vec<(u32, String, Vec<u8>)> = Vec::new();
        let icon_byte_count = (ICON_SLOT_PX * ICON_SLOT_PX * 4) as usize;

        // Reserve the slot range we're about to fill before
        // decoding so the parallel decode pass can run without
        // touching `self`. Track entries that fit the atlas;
        // those that don't are dropped with a warning.
        let mut jobs: Vec<(u32, std::path::PathBuf, String)> = Vec::with_capacity(budget);
        for _ in 0..budget {
            if self.next_icon_idx >= self.pending_icon_paths.len() {
                break;
            }
            let (path, name) = {
                let entry = &self.pending_icon_paths[self.next_icon_idx];
                (entry.0.clone(), entry.1.clone())
            };
            self.next_icon_idx += 1;
            let slot = self.loaded_icons as u32;
            // Charge the slot regardless of outcome — a failed
            // icon still consumes its slot so progress
            // monotonically advances and slot indices stay
            // aligned with the originally discovered order.
            self.loaded_icons += 1;

            let col = slot % ICON_COLS;
            let row = slot / ICON_COLS;
            let x0 = col * ICON_SLOT_PX;
            let y0 = ICON_REGION_Y + row * ICON_SLOT_PX;
            if x0 + ICON_SLOT_PX > self.atlas_width || y0 + ICON_SLOT_PX > self.atlas_height {
                log::warn!("overlay: atlas full — dropping icon {name} (slot {slot})");
                continue;
            }
            jobs.push((slot, path, name));
        }

        // Decode + resize in parallel. PNG decompression and the
        // Catmull-Rom resize are both CPU-bound and embarrassingly
        // parallel; with ~330 icons this drops the loading screen
        // from O(seconds) to O(hundreds of ms) on a multi-core box.
        use rayon::prelude::*;
        let decoded_par: Vec<Option<(u32, String, Vec<u8>)>> = jobs
            .into_par_iter()
            .map(|(slot, path, name)| {
                let img = match image::open(&path) {
                    Ok(img) => img,
                    Err(e) => {
                        log::warn!("overlay: failed to load icon {}: {e}", path.display(),);
                        return None;
                    }
                };
                let resized = img
                    .resize_exact(ICON_SLOT_PX, ICON_SLOT_PX, FilterType::CatmullRom)
                    .to_rgba8();
                Some((slot, name, resized.into_raw()))
            })
            .collect();
        for entry in decoded_par.into_iter().flatten() {
            decoded.push(entry);
        }

        if decoded.is_empty() {
            return Ok((self.loaded_icons, self.total_icons));
        }

        // Single staging buffer holding every icon's pixels back
        // to back. Each icon owns a contiguous slice; the buffer
        // offset of icon `i` is `i * icon_byte_count`.
        let mut staging_bytes: Vec<u8> = Vec::with_capacity(decoded.len() * icon_byte_count);
        for (_, _, pixels) in &decoded {
            staging_bytes.extend_from_slice(pixels);
        }
        let staging = buffer::create_host_buffer(
            device,
            allocator,
            &staging_bytes,
            vk::BufferUsageFlags::TRANSFER_SRC,
            "icon_staging_batch",
        )?;

        let cmd = Self::begin_single_time_commands(device, command_pool)?;
        let subresource = vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        };
        unsafe {
            // One SHADER_READ_ONLY -> TRANSFER_DST barrier covers
            // every copy in this batch.
            let to_dst = vk::ImageMemoryBarrier::default()
                .image(self.font_image)
                .old_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .src_access_mask(vk::AccessFlags::SHADER_READ)
                .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .subresource_range(subresource);
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[to_dst],
            );

            // Build one BufferImageCopy per decoded icon. They all
            // share the same source buffer, just different offsets
            // and image_offset rects.
            let regions: Vec<vk::BufferImageCopy> = decoded
                .iter()
                .enumerate()
                .map(|(i, (slot, _, _))| {
                    let col = slot % ICON_COLS;
                    let row = slot / ICON_COLS;
                    let x0 = col * ICON_SLOT_PX;
                    let y0 = ICON_REGION_Y + row * ICON_SLOT_PX;
                    vk::BufferImageCopy::default()
                        .buffer_offset((i * icon_byte_count) as u64)
                        .image_subresource(vk::ImageSubresourceLayers {
                            aspect_mask: vk::ImageAspectFlags::COLOR,
                            mip_level: 0,
                            base_array_layer: 0,
                            layer_count: 1,
                        })
                        .image_offset(vk::Offset3D {
                            x: x0 as i32,
                            y: y0 as i32,
                            z: 0,
                        })
                        .image_extent(vk::Extent3D {
                            width: ICON_SLOT_PX,
                            height: ICON_SLOT_PX,
                            depth: 1,
                        })
                })
                .collect();
            device.cmd_copy_buffer_to_image(
                cmd,
                staging.buffer,
                self.font_image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &regions,
            );

            // TRANSFER_DST -> SHADER_READ_ONLY
            let to_read = vk::ImageMemoryBarrier::default()
                .image(self.font_image)
                .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::SHADER_READ)
                .subresource_range(subresource);
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[to_read],
            );
        }
        Self::end_single_time_commands(device, command_pool, queue, cmd)?;

        let mut staging = staging;
        staging.cleanup(device, allocator);

        // Now that the GPU upload is committed, register the UVs
        // so subsequent draws can resolve these icons.
        let mut registry = self.icon_uv.write().unwrap();
        for (slot, name, _) in decoded {
            let col = slot % ICON_COLS;
            let row = slot / ICON_COLS;
            let x0 = col * ICON_SLOT_PX;
            let y0 = ICON_REGION_Y + row * ICON_SLOT_PX;
            let u0 = x0 as f32 / self.atlas_width as f32;
            let v0 = y0 as f32 / self.atlas_height as f32;
            let u1 = (x0 + ICON_SLOT_PX) as f32 / self.atlas_width as f32;
            let v1 = (y0 + ICON_SLOT_PX) as f32 / self.atlas_height as f32;
            registry.insert(name, [u0, v0, u1, v1]);
        }

        Ok((self.loaded_icons, self.total_icons))
    }

    /// Upload overlay batch to GPU. Call once per frame before recording.
    pub fn upload(
        &mut self,
        frame: usize,
        device: &ash::Device,
        allocator: &std::sync::Arc<std::sync::Mutex<gpu_allocator::vulkan::Allocator>>,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        batch: &OverlayBatch,
    ) -> Result<()> {
        // Free old buffers in this frame slot. Safe because draw_frame waited
        // on this frame's fence before invoking upload.
        if let Some(mut vb) = self.vertex_buffers[frame].take() {
            vb.cleanup(device, allocator);
        }
        if let Some(mut ib) = self.index_buffers[frame].take() {
            ib.cleanup(device, allocator);
        }

        if batch.is_empty() {
            self.index_counts[frame] = 0;
            return Ok(());
        }

        self.vertex_buffers[frame] = Some(buffer::create_device_local_buffer(
            device,
            allocator,
            queue,
            command_pool,
            &batch.vertices,
            vk::BufferUsageFlags::VERTEX_BUFFER,
            "overlay_vb",
        )?);

        self.index_buffers[frame] = Some(buffer::create_device_local_buffer(
            device,
            allocator,
            queue,
            command_pool,
            &batch.indices,
            vk::BufferUsageFlags::INDEX_BUFFER,
            "overlay_ib",
        )?);

        self.index_counts[frame] = batch.indices.len() as u32;
        Ok(())
    }

    /// Record overlay draw commands into the current render pass.
    pub fn record(&self, frame: usize, device: &ash::Device, cmd: vk::CommandBuffer) {
        let count = self.index_counts[frame];
        if count == 0 {
            return;
        }
        let vb = match &self.vertex_buffers[frame] {
            Some(b) => b.buffer,
            None => return,
        };
        let ib = match &self.index_buffers[frame] {
            Some(b) => b.buffer,
            None => return,
        };

        unsafe {
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline_layout,
                0,
                &[self.descriptor_set],
                &[],
            );
            device.cmd_bind_vertex_buffers(cmd, 0, &[vb], &[0]);
            device.cmd_bind_index_buffer(cmd, ib, 0, vk::IndexType::UINT32);
            device.cmd_draw_indexed(cmd, count, 1, 0, 0, 0);
        }
    }

    pub fn recreate_pipeline(
        &mut self,
        device: &ash::Device,
        render_pass: vk::RenderPass,
        extent: vk::Extent2D,
        shader_dir: &std::path::Path,
    ) -> Result<()> {
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
        }
        let (pipeline, layout) = Self::create_pipeline(
            device,
            render_pass,
            extent,
            self.descriptor_set_layout,
            shader_dir,
        )?;
        self.pipeline = pipeline;
        self.pipeline_layout = layout;
        Ok(())
    }

    fn create_pipeline(
        device: &ash::Device,
        render_pass: vk::RenderPass,
        extent: vk::Extent2D,
        descriptor_set_layout: vk::DescriptorSetLayout,
        shader_dir: &std::path::Path,
    ) -> Result<(vk::Pipeline, vk::PipelineLayout)> {
        let vert_src = std::fs::read_to_string(shader_dir.join("overlay.vert"))?;
        let frag_src = std::fs::read_to_string(shader_dir.join("overlay.frag"))?;

        let vert_spv =
            hot_reload::compile_glsl(&vert_src, "overlay.vert", shaderc::ShaderKind::Vertex)?;
        let frag_spv =
            hot_reload::compile_glsl(&frag_src, "overlay.frag", shaderc::ShaderKind::Fragment)?;

        let vert_module = crate::vulkan::pipeline::create_shader_module(device, &vert_spv)?;
        let frag_module = crate::vulkan::pipeline::create_shader_module(device, &frag_spv)?;

        let entry_name = c"main";
        let shader_stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(vert_module)
                .name(entry_name),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(frag_module)
                .name(entry_name),
        ];

        let binding_desc = [OverlayVertex::binding_description()];
        let attr_descs = OverlayVertex::attribute_descriptions();

        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&binding_desc)
            .vertex_attribute_descriptions(&attr_descs);

        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);

        let viewport = vk::Viewport {
            x: 0.0,
            y: 0.0,
            width: extent.width as f32,
            height: extent.height as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        };
        let scissor = vk::Rect2D {
            offset: vk::Offset2D { x: 0, y: 0 },
            extent,
        };
        let viewport_state = vk::PipelineViewportStateCreateInfo::default()
            .viewports(std::slice::from_ref(&viewport))
            .scissors(std::slice::from_ref(&scissor));

        let rasterizer = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .line_width(1.0)
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE);

        let multisampling = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);

        let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(false)
            .depth_write_enable(false);

        let color_blend_attachment = vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA)
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ZERO)
            .alpha_blend_op(vk::BlendOp::ADD);

        let color_blending = vk::PipelineColorBlendStateCreateInfo::default()
            .attachments(std::slice::from_ref(&color_blend_attachment));

        let set_layouts = [descriptor_set_layout];
        let layout_info = vk::PipelineLayoutCreateInfo::default().set_layouts(&set_layouts);
        let pipeline_layout = unsafe { device.create_pipeline_layout(&layout_info, None)? };

        let pipeline_info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&shader_stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport_state)
            .rasterization_state(&rasterizer)
            .multisample_state(&multisampling)
            .depth_stencil_state(&depth_stencil)
            .color_blend_state(&color_blending)
            .layout(pipeline_layout)
            .render_pass(render_pass)
            .subpass(0);

        let pipeline = unsafe {
            device
                .create_graphics_pipelines(vk::PipelineCache::null(), &[pipeline_info], None)
                .map_err(|(_, e)| e)?[0]
        };

        unsafe {
            device.destroy_shader_module(vert_module, None);
            device.destroy_shader_module(frag_module, None);
        }

        Ok((pipeline, pipeline_layout))
    }

    fn create_descriptor_set_layout(device: &ash::Device) -> Result<vk::DescriptorSetLayout> {
        let binding = vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT);

        let layout_info =
            vk::DescriptorSetLayoutCreateInfo::default().bindings(std::slice::from_ref(&binding));

        let layout = unsafe { device.create_descriptor_set_layout(&layout_info, None)? };
        Ok(layout)
    }

    fn create_descriptor_pool(device: &ash::Device) -> Result<vk::DescriptorPool> {
        let pool_size = vk::DescriptorPoolSize {
            ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
            descriptor_count: 1,
        };
        let pool_info = vk::DescriptorPoolCreateInfo::default()
            .pool_sizes(std::slice::from_ref(&pool_size))
            .max_sets(1);

        let pool = unsafe { device.create_descriptor_pool(&pool_info, None)? };
        Ok(pool)
    }

    fn allocate_descriptor_set(
        device: &ash::Device,
        pool: vk::DescriptorPool,
        layout: vk::DescriptorSetLayout,
    ) -> Result<vk::DescriptorSet> {
        let layouts = [layout];
        let alloc_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(pool)
            .set_layouts(&layouts);

        let sets = unsafe { device.allocate_descriptor_sets(&alloc_info)? };
        Ok(sets[0])
    }

    fn update_descriptor_set(
        device: &ash::Device,
        set: vk::DescriptorSet,
        image_view: vk::ImageView,
        sampler: vk::Sampler,
    ) {
        let image_info = vk::DescriptorImageInfo::default()
            .sampler(sampler)
            .image_view(image_view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);

        let write = vk::WriteDescriptorSet::default()
            .dst_set(set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .image_info(std::slice::from_ref(&image_info));

        unsafe {
            device.update_descriptor_sets(&[write], &[]);
        }
    }

    fn create_font_image(
        device: &ash::Device,
        allocator: &std::sync::Arc<std::sync::Mutex<gpu_allocator::vulkan::Allocator>>,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        data: &[u8],
        width: u32,
        height: u32,
    ) -> Result<(vk::Image, gpu_allocator::vulkan::Allocation)> {
        use gpu_allocator::vulkan::{AllocationCreateDesc, AllocationScheme};
        use gpu_allocator::MemoryLocation;

        // Create staging buffer
        let staging = buffer::create_host_buffer(
            device,
            allocator,
            data,
            vk::BufferUsageFlags::TRANSFER_SRC,
            "font_staging",
        )?;

        // Create image
        let image_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk::Format::R8G8B8A8_UNORM)
            .extent(vk::Extent3D {
                width,
                height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);

        let image = unsafe { device.create_image(&image_info, None)? };
        let reqs = unsafe { device.get_image_memory_requirements(image) };

        let allocation = allocator.lock().unwrap().allocate(&AllocationCreateDesc {
            name: "font_atlas",
            requirements: reqs,
            location: MemoryLocation::GpuOnly,
            linear: false,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        })?;

        unsafe { device.bind_image_memory(image, allocation.memory(), allocation.offset())? };

        // Copy staging → image
        let cmd_buf = Self::begin_single_time_commands(device, command_pool)?;
        unsafe {
            // Transition to TRANSFER_DST
            let barrier = vk::ImageMemoryBarrier::default()
                .image(image)
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .src_access_mask(vk::AccessFlags::empty())
                .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                });
            device.cmd_pipeline_barrier(
                cmd_buf,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[barrier],
            );

            // Copy
            let region = vk::BufferImageCopy::default()
                .image_subresource(vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: 0,
                    base_array_layer: 0,
                    layer_count: 1,
                })
                .image_extent(vk::Extent3D {
                    width,
                    height,
                    depth: 1,
                });
            device.cmd_copy_buffer_to_image(
                cmd_buf,
                staging.buffer,
                image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[region],
            );

            // Transition to SHADER_READ_ONLY
            let barrier = vk::ImageMemoryBarrier::default()
                .image(image)
                .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::SHADER_READ)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                });
            device.cmd_pipeline_barrier(
                cmd_buf,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[barrier],
            );
        }
        Self::end_single_time_commands(device, command_pool, queue, cmd_buf)?;

        // Free staging buffer
        let mut staging = staging;
        staging.cleanup(device, allocator);

        Ok((image, allocation))
    }

    fn create_image_view(device: &ash::Device, image: vk::Image) -> Result<vk::ImageView> {
        let view_info = vk::ImageViewCreateInfo::default()
            .image(image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(vk::Format::R8G8B8A8_UNORM)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            });
        let view = unsafe { device.create_image_view(&view_info, None)? };
        Ok(view)
    }

    fn create_sampler(device: &ash::Device) -> Result<vk::Sampler> {
        // NEAREST for both filters \u2014 the bitmap font glyphs sit
        // edge-to-edge in the atlas with no padding, so LINEAR
        // would bleed neighbouring glyphs into each other. Icons
        // are pre-resized to their slot size, so NEAREST renders
        // them 1:1 without aliasing as long as the HUD slot size
        // matches `ICON_SLOT_PX`.
        let sampler_info = vk::SamplerCreateInfo::default()
            .mag_filter(vk::Filter::NEAREST)
            .min_filter(vk::Filter::NEAREST)
            .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .mipmap_mode(vk::SamplerMipmapMode::NEAREST);
        let sampler = unsafe { device.create_sampler(&sampler_info, None)? };
        Ok(sampler)
    }

    fn begin_single_time_commands(
        device: &ash::Device,
        pool: vk::CommandPool,
    ) -> Result<vk::CommandBuffer> {
        let alloc_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        let cmd = unsafe { device.allocate_command_buffers(&alloc_info)? }[0];
        let begin_info = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        unsafe { device.begin_command_buffer(cmd, &begin_info)? };
        Ok(cmd)
    }

    fn end_single_time_commands(
        device: &ash::Device,
        pool: vk::CommandPool,
        queue: vk::Queue,
        cmd: vk::CommandBuffer,
    ) -> Result<()> {
        unsafe {
            device.end_command_buffer(cmd)?;
            let submit_info = vk::SubmitInfo::default().command_buffers(std::slice::from_ref(&cmd));
            device.queue_submit(queue, &[submit_info], vk::Fence::null())?;
            device.queue_wait_idle(queue)?;
            device.free_command_buffers(pool, &[cmd]);
        }
        Ok(())
    }

    fn free_buffers(
        &mut self,
        device: &ash::Device,
        allocator: &std::sync::Arc<std::sync::Mutex<gpu_allocator::vulkan::Allocator>>,
    ) {
        for slot in self.vertex_buffers.iter_mut() {
            if let Some(mut vb) = slot.take() {
                vb.cleanup(device, allocator);
            }
        }
        for slot in self.index_buffers.iter_mut() {
            if let Some(mut ib) = slot.take() {
                ib.cleanup(device, allocator);
            }
        }
    }

    pub fn cleanup(
        &mut self,
        device: &ash::Device,
        allocator: &std::sync::Arc<std::sync::Mutex<gpu_allocator::vulkan::Allocator>>,
    ) {
        self.free_buffers(device, allocator);
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
            device.destroy_descriptor_pool(self.descriptor_pool, None);
            device.destroy_descriptor_set_layout(self.descriptor_set_layout, None);
            device.destroy_sampler(self.font_sampler, None);
            device.destroy_image_view(self.font_image_view, None);
            device.destroy_image(self.font_image, None);
        }
        if let Some(alloc) = self.font_allocation.take() {
            allocator.lock().unwrap().free(alloc).ok();
        }
    }
}

/// Side length of one icon slot in the overlay atlas, in pixels.
const ICON_SLOT_PX: u32 = 48;
/// How many icons fit per atlas row.
const ICON_COLS: u32 = 4;

/// Discover every `*.png` under `assets/icons/` recursively and
/// return their `(path, key)` pairs sorted by key. The key is the
/// path relative to `assets/icons/` with the `.png` extension
/// stripped and `\` rewritten to `/` so look-ups work the same
/// on Windows and POSIX.
///
/// Sorting makes slot indices deterministic across runs (read-dir
/// order isn't guaranteed); the slot index doesn't matter for
/// look-up since names are the key, but stable layout helps when
/// debugging the atlas image. A missing/unreadable directory
/// yields an empty list — the engine still boots, HUD just falls
/// back.
///
/// Subdirectories are scoped into the key (e.g.
/// `assets/icons/loot/Boots/Boots_1.png` ⇒ `loot/Boots/Boots_1`)
/// so collisions between e.g. flat ability icons (`Hunter_3`)
/// and slot-scoped item icons (`loot/Boots/Boots_1`) are
/// impossible by construction.
fn discover_icon_paths() -> Vec<(std::path::PathBuf, String)> {
    let base_dir = std::path::Path::new("assets").join("icons");
    let mut out: Vec<(std::path::PathBuf, String)> = Vec::new();
    if !base_dir.exists() {
        log::warn!(
            "overlay: icon dir {} not present; HUD will fall back to placeholders",
            base_dir.display(),
        );
        return out;
    }
    let mut stack: Vec<std::path::PathBuf> = vec![base_dir.clone()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(it) => it,
            Err(e) => {
                log::warn!("overlay: icon dir {} not readable ({e})", dir.display());
                continue;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let is_png = path
                .extension()
                .and_then(|s| s.to_str())
                .map(|s| s.eq_ignore_ascii_case("png"))
                .unwrap_or(false);
            if !is_png {
                continue;
            }
            let rel = match path.strip_prefix(&base_dir) {
                Ok(r) => r,
                Err(_) => continue,
            };
            // Drop the .png extension and normalise separators.
            let mut key = rel.with_extension("").to_string_lossy().into_owned();
            if std::path::MAIN_SEPARATOR != '/' {
                key = key.replace(std::path::MAIN_SEPARATOR, "/");
            }
            out.push((path, key));
        }
    }
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out
}

/// Compute the overlay-atlas height needed to fit `icon_count`
/// icon slots below the font region. Always at least
/// `OVERLAY_ATLAS_SIZE` so the atlas stays square at minimum.
/// Width is fixed; only height grows.
fn compute_atlas_height(icon_count: u32) -> u32 {
    use crate::renderer::font::{ICON_REGION_Y, OVERLAY_ATLAS_SIZE};
    let rows = (icon_count + ICON_COLS - 1) / ICON_COLS;
    let needed = ICON_REGION_Y + rows * ICON_SLOT_PX;
    needed.max(OVERLAY_ATLAS_SIZE)
}

// ─── DrawList bridge ───────────────────────────────────────────
//
// `rift_ui_im::DrawList` is what widgets emit pixels through;
// keeping the impl here means the UI crate doesn't have to know
// the batch type. Methods just forward — the trait surface was
// designed to match `OverlayBatch`'s existing signatures.
impl rift_ui_im::DrawList for OverlayBatch {
    fn rect_px(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: [f32; 4],
        screen_w: f32,
        screen_h: f32,
    ) {
        OverlayBatch::rect_px(self, x, y, w, h, color, screen_w, screen_h);
    }

    fn rounded_rect_px(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        color: [f32; 4],
        screen_w: f32,
        screen_h: f32,
    ) {
        OverlayBatch::rounded_rect_px(self, x, y, w, h, radius, color, screen_w, screen_h);
    }

    fn rect_px_grad_v(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        top: [f32; 4],
        bot: [f32; 4],
        screen_w: f32,
        screen_h: f32,
    ) {
        OverlayBatch::rect_px_grad_v(self, x, y, w, h, top, bot, screen_w, screen_h);
    }

    fn rounded_rect_px_grad_v(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        top: [f32; 4],
        bot: [f32; 4],
        screen_w: f32,
        screen_h: f32,
    ) {
        OverlayBatch::rounded_rect_px_grad_v(
            self, x, y, w, h, radius, top, bot, screen_w, screen_h,
        );
    }

    fn rect_px_grad4(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        tl: [f32; 4],
        tr: [f32; 4],
        bl: [f32; 4],
        br: [f32; 4],
        screen_w: f32,
        screen_h: f32,
    ) {
        OverlayBatch::rect_px_grad4(self, x, y, w, h, tl, tr, bl, br, screen_w, screen_h);
    }

    fn rounded_rect_px_radial(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        edge: [f32; 4],
        centre: [f32; 4],
        screen_w: f32,
        screen_h: f32,
    ) {
        OverlayBatch::rounded_rect_px_radial(
            self, x, y, w, h, radius, edge, centre, screen_w, screen_h,
        );
    }

    fn rounded_rect_px_radial_noisy(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        edge: [f32; 4],
        centre: [f32; 4],
        screen_w: f32,
        screen_h: f32,
    ) {
        OverlayBatch::rounded_rect_px_radial_noisy(
            self, x, y, w, h, radius, edge, centre, screen_w, screen_h,
        );
    }

    fn rounded_outline_px(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        thickness: f32,
        color: [f32; 4],
        screen_w: f32,
        screen_h: f32,
    ) {
        OverlayBatch::rounded_outline_px(
            self, x, y, w, h, radius, thickness, color, screen_w, screen_h,
        );
    }

    fn line_px(
        &mut self,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        thickness: f32,
        color: [f32; 4],
        screen_w: f32,
        screen_h: f32,
    ) {
        OverlayBatch::line_px(self, x0, y0, x1, y1, thickness, color, screen_w, screen_h);
    }

    fn glow_disc(
        &mut self,
        cx: f32,
        cy: f32,
        radius: f32,
        halo: f32,
        color: [f32; 4],
        screen_w: f32,
        screen_h: f32,
    ) {
        OverlayBatch::glow_disc(self, cx, cy, radius, halo, color, screen_w, screen_h);
    }

    fn glow_line(
        &mut self,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        core_thickness: f32,
        halo_extent: f32,
        color: [f32; 4],
        screen_w: f32,
        screen_h: f32,
    ) {
        OverlayBatch::glow_line(
            self,
            x0,
            y0,
            x1,
            y1,
            core_thickness,
            halo_extent,
            color,
            screen_w,
            screen_h,
        );
    }

    fn text(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        size: f32,
        color: [f32; 4],
        screen_w: f32,
        screen_h: f32,
    ) -> f32 {
        OverlayBatch::text(self, text, x, y, size, color, screen_w, screen_h)
    }

    fn measure_text(&self, text: &str, size: f32) -> f32 {
        OverlayBatch::measure_text(self, text, size)
    }

    fn icon(
        &mut self,
        name: &str,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        tint: [f32; 4],
        screen_w: f32,
        screen_h: f32,
    ) -> bool {
        OverlayBatch::icon(self, name, x, y, w, h, tint, screen_w, screen_h)
    }
}
