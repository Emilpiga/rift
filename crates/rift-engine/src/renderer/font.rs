//! TTF-rasterised UI font (PT Serif) packed into the overlay
//! atlas.
//!
//! The overlay pipeline samples a single RGBA atlas: the top
//! `ICON_REGION_Y` rows hold rasterised font glyphs (and the
//! solid-white pixel at (0,0) used for flat-colour rects);
//! everything below is the icon region the renderer streams
//! into at startup.
//!
//! Why one big raster + scale at draw time rather than
//! multiple pre-baked sizes: a single 36-px raster of PT Serif
//! covers every UI token (`size_sm 12 → size_xl 28`, all
//! multiplied by the per-frame scale), packs into the
//! 512×320 font region with room to spare, and looks crisp at
//! all our display sizes thanks to the shader's bilinear
//! sampler. Authoring multiple atlases per font size would
//! multiply the upload + descriptor surface for marginal gain.
//!
//! The on-disk TTF is bundled at compile time via
//! `include_bytes!` so a missing font file is a build error,
//! not a runtime panic.

use std::collections::HashMap;

use fontdue::{Font, FontSettings};

/// PT Serif Regular bytes baked into the binary.
const FONT_BYTES: &[u8] = include_bytes!("../../../../assets/fonts/PT_Serif/PTSerif-Regular.ttf");

/// Side length of the combined overlay atlas in pixels. The
/// font region sits in the top-left up to [`ICON_REGION_Y`];
/// icons live below.
pub const OVERLAY_ATLAS_SIZE: u32 = 512;

/// y-coordinate where the icon region starts. Everything above
/// belongs to the font raster (and the solid-white pixel at
/// (0,0) used by flat-colour vertices).
pub const ICON_REGION_Y: u32 = 320;

/// Pixel size we rasterise the font at. Picked above the
/// largest UI token (`size_xl = 28` × max scale ≈ 1.5 → 42 px,
/// rounded down a touch) so glyphs sample crisply even when
/// the bilinear sampler upsizes them. Going higher just costs
/// atlas area without visible gain.
const RASTER_PX: f32 = 36.0;

/// 1-px padding around every glyph in the atlas so bilinear
/// sampling at glyph edges doesn't bleed across into the
/// neighbouring glyph's footprint.
const GLYPH_PAD: u32 = 1;

/// Per-glyph metadata. UVs are normalised against the *whole*
/// atlas; `w_px` / `h_px` are the rasterised pixel dimensions
/// of the glyph at [`RASTER_PX`]; `x_offset` / `y_offset` are
/// the bearings from the layout pen position to the glyph
/// bitmap's top-left at the same raster size; `advance` is the
/// horizontal pen advance after drawing the glyph, also in
/// raster pixels. Render-time code multiplies all four by
/// `display_size / raster_px` to scale to the requested size.
#[derive(Debug, Clone, Copy)]
pub struct GlyphInfo {
    pub u0: f32,
    pub v0: f32,
    pub u1: f32,
    pub v1: f32,
    pub w_px: f32,
    pub h_px: f32,
    pub x_offset: f32,
    pub y_offset: f32,
    pub advance: f32,
}

/// TTF-backed overlay font. Built once at startup. Name is
/// preserved for backward compatibility with the engine call
/// sites that already reference `BitmapFont`.
pub struct BitmapFont {
    /// Pixel size we rasterised at; render code reads this to
    /// compute per-character draw scale (`size / raster_px`).
    pub raster_px: f32,
    /// Vertical line height in raster pixels (ascent − descent
    /// + line gap). Render code can use this to walk
    /// multi-line layouts.
    pub line_height: f32,
    /// Ascent in raster pixels — distance from the layout y
    /// origin down to the baseline. Glyph y-offsets are
    /// expressed relative to the bitmap top, so the render
    /// path uses this to align baselines across glyphs of
    /// differing heights.
    pub ascent: f32,
    /// Full overlay atlas dimensions (font + icon region).
    pub atlas_width: u32,
    pub atlas_height: u32,
    /// Per-character metrics indexed by `char`.
    glyphs: HashMap<char, GlyphInfo>,
    /// Rasterised glyph bitmaps, kept around so [`atlas_data`]
    /// can re-emit the atlas image on demand. Tuple is
    /// `(x, y, w, h, mask)` with `mask` row-major 8-bit
    /// coverage (0..=255).
    rasters: Vec<(u32, u32, u32, u32, Vec<u8>)>,
}

impl BitmapFont {
    pub fn new() -> Self {
        Self::with_atlas_size(OVERLAY_ATLAS_SIZE, OVERLAY_ATLAS_SIZE)
    }

    /// Construct a font that paints into an atlas of the given
    /// dimensions. The font region itself uses the top
    /// `ICON_REGION_Y` rows; growing the atlas height just
    /// creates more room for the icon region below. Width is
    /// fixed by [`OVERLAY_ATLAS_SIZE`].
    pub fn with_atlas_size(atlas_width: u32, atlas_height: u32) -> Self {
        let font = Font::from_bytes(FONT_BYTES, FontSettings::default())
            .expect("PT Serif TTF failed to parse — bundled asset corrupted?");

        // Line metrics at our reference raster size.
        let line_metrics = font
            .horizontal_line_metrics(RASTER_PX)
            .expect("font lacks horizontal line metrics");
        let ascent = line_metrics.ascent;
        let line_height =
            (line_metrics.ascent - line_metrics.descent + line_metrics.line_gap).ceil();
        // Effective ascent used to place glyphs against the
        // caller's `y` argument. We treat `y` as the top of an
        // em-box of height `size` (display px), with the
        // baseline at `y + size * (ascent / line_height)`.
        // Baking that ratio into the per-glyph y_offset lets
        // the render path keep its simple `y + y_offset*scale`
        // formula while making `size` align with the visible
        // text height — otherwise glyphs sit ~5–15 % below
        // a vertically-centred rect because the raw ascent
        // exceeds `size` and the baseline falls past centre.
        let effective_ascent = ascent * RASTER_PX / line_height.max(1.0);

        let mut glyphs = HashMap::with_capacity(96);
        let mut rasters = Vec::with_capacity(96);

        // Reserve the top-left pixel for the solid-white
        // sample so flat-colour rects keep working unchanged.
        // The packer starts a few pixels in.
        let mut pen_x: u32 = 4 + GLYPH_PAD;
        let mut pen_y: u32 = GLYPH_PAD;
        let mut row_h: u32 = 0;

        // Rasterise printable ASCII (32..=126). Going wider
        // (Latin-1, currency, arrows) is a one-line change.
        for code in 32u32..=126 {
            let ch = match char::from_u32(code) {
                Some(c) => c,
                None => continue,
            };
            let (metrics, bitmap) = font.rasterize(ch, RASTER_PX);
            let gw = metrics.width as u32;
            let gh = metrics.height as u32;

            // Zero-size glyph (e.g. space) — record metrics
            // only, no atlas footprint.
            if gw == 0 || gh == 0 {
                glyphs.insert(
                    ch,
                    GlyphInfo {
                        u0: 0.0,
                        v0: 0.0,
                        u1: 0.0,
                        v1: 0.0,
                        w_px: 0.0,
                        h_px: 0.0,
                        x_offset: 0.0,
                        y_offset: 0.0,
                        advance: metrics.advance_width,
                    },
                );
                continue;
            }

            // Wrap to a new shelf when the glyph wouldn't fit
            // on the current row.
            if pen_x + gw + GLYPH_PAD > atlas_width {
                pen_x = GLYPH_PAD;
                pen_y += row_h + GLYPH_PAD;
                row_h = 0;
            }
            // If we'd overflow the font region, drop the rest
            // (extremely unlikely with PT Serif at 36 px in a
            // 320-tall region; logging keeps the failure
            // visible if a future font change pushes us over).
            if pen_y + gh + GLYPH_PAD > ICON_REGION_Y {
                log::warn!(
                    "font: ran out of atlas room at glyph U+{:04X} '{}' \
                     (atlas font region is {}x{}, glyph would land at y={})",
                    code,
                    ch,
                    atlas_width,
                    ICON_REGION_Y,
                    pen_y + gh,
                );
                break;
            }

            let u0 = pen_x as f32 / atlas_width as f32;
            let v0 = pen_y as f32 / atlas_height as f32;
            let u1 = (pen_x + gw) as f32 / atlas_width as f32;
            let v1 = (pen_y + gh) as f32 / atlas_height as f32;

            // fontdue's `ymin` is the baseline-relative bottom
            // of the bitmap (positive = above baseline). We
            // flip into a top-down `y_offset` measured from
            // the layout origin (top of the size-tall em-box)
            // so render code can do
            //   pos.y + y_offset_scaled
            // to land the glyph correctly. Using
            // `effective_ascent` (ascent normalised against
            // line_height) keeps the visible text inside the
            // requested `size` rather than overflowing below.
            let x_offset = metrics.xmin as f32;
            let y_offset = effective_ascent - metrics.ymin as f32 - gh as f32;

            glyphs.insert(
                ch,
                GlyphInfo {
                    u0,
                    v0,
                    u1,
                    v1,
                    w_px: gw as f32,
                    h_px: gh as f32,
                    x_offset,
                    y_offset,
                    advance: metrics.advance_width,
                },
            );
            rasters.push((pen_x, pen_y, gw, gh, bitmap));

            pen_x += gw + GLYPH_PAD;
            row_h = row_h.max(gh);
        }

        Self {
            raster_px: RASTER_PX,
            line_height,
            ascent,
            atlas_width,
            atlas_height,
            glyphs,
            rasters,
        }
    }

    /// Per-character glyph metrics. Returns `None` for
    /// characters not in the rasterised set.
    pub fn glyph(&self, ch: char) -> Option<GlyphInfo> {
        self.glyphs.get(&ch).copied()
    }

    /// Pen advance for a single character at raster scale, in
    /// raster pixels. Falls back to a half-em for unknown
    /// glyphs so cursor advance keeps moving on missing chars.
    pub fn advance(&self, ch: char) -> f32 {
        self.glyphs
            .get(&ch)
            .map(|g| g.advance)
            .unwrap_or(self.raster_px * 0.5)
    }

    /// Generate the combined overlay atlas pixel data as RGBA8.
    /// The font region stores glyph masks as
    /// `(255, 255, 255, mask)`; the icon region (`y >=
    /// ICON_REGION_Y`) is left zeroed for the renderer to fill
    /// later via sub-region uploads.
    pub fn atlas_data(&self) -> Vec<u8> {
        let w = self.atlas_width as usize;
        let h = self.atlas_height as usize;
        let mut data = vec![0u8; w * h * 4];

        // Solid-white 4×4 patch at (0, 0) — flat-colour rects
        // (and the noisy radial overlay primitive) sample
        // this. The 4×4 footprint protects against bilinear
        // sampling picking up a transparent neighbour at
        // sub-pixel positions.
        for y in 0..4usize.min(h) {
            for x in 0..4usize.min(w) {
                let dst = (y * w + x) * 4;
                data[dst] = 255;
                data[dst + 1] = 255;
                data[dst + 2] = 255;
                data[dst + 3] = 255;
            }
        }

        // Stamp each rasterised glyph as
        // (255, 255, 255, coverage).
        for (px, py, gw, gh, bitmap) in &self.rasters {
            for gy in 0..*gh as usize {
                for gx in 0..*gw as usize {
                    let mask = bitmap[gy * (*gw as usize) + gx];
                    if mask == 0 {
                        continue;
                    }
                    let x = *px as usize + gx;
                    let y = *py as usize + gy;
                    if x >= w || y >= h {
                        continue;
                    }
                    let dst = (y * w + x) * 4;
                    data[dst] = 255;
                    data[dst + 1] = 255;
                    data[dst + 2] = 255;
                    data[dst + 3] = mask;
                }
            }
        }

        data
    }
}
