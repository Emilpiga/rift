//! TTF-rasterised UI fonts packed into the overlay atlas: PT
//! Serif for body copy and Share Tech for headers / panel
//! titles.
//!
//! The overlay pipeline samples a single RGBA atlas: the top
//! `ICON_REGION_Y` rows hold rasterised font glyphs (and the
//! solid-white pixel at (0,0) used for flat-colour rects);
//! everything below is the icon region the renderer streams
//! into at startup.
//!
//! The on-disk TTFs are bundled at compile time via
//! `include_bytes!` so a missing font file is a build error,
//! not a runtime panic.

use std::collections::HashMap;

use fontdue::{Font, FontSettings};

/// PT Serif Regular — body UI text.
const BODY_FONT_BYTES: &[u8] =
    include_bytes!("../../../../assets/fonts/PT_Serif/PTSerif-Regular.ttf");
/// Share Tech — panel titles, section headers, HUD chrome labels.
const HEADER_FONT_BYTES: &[u8] =
    include_bytes!("../../../../assets/fonts/Share_Tech/ShareTech-Regular.ttf");

/// Side length of the combined overlay atlas in pixels. The
/// font region sits in the top-left up to [`ICON_REGION_Y`];
/// icons live below.
pub const OVERLAY_ATLAS_SIZE: u32 = 512;

/// y-coordinate where the icon region starts. Everything above
/// belongs to the font raster (and the solid-white pixel at
/// (0,0) used by flat-colour vertices). Must fit **both** body and
/// header Latin-1 shelves (32–126 + 160–255) at [`RASTER_PX`]
/// on a 512px-wide atlas (~20 rows × 65px per face).
pub const ICON_REGION_Y: u32 = 2048;

/// Pixel size we rasterise each face at.
const RASTER_PX: f32 = 64.0;

const GLYPH_PAD: u32 = 1;

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

/// One packed typeface in the shared atlas.
#[derive(Debug)]
struct PackedFace {
    glyphs: HashMap<char, GlyphInfo>,
    rasters: Vec<(u32, u32, u32, u32, Vec<u8>)>,
    #[allow(dead_code)]
    line_height: f32,
    #[allow(dead_code)]
    ascent: f32,
}

impl PackedFace {
    fn pack(
        font: &Font,
        glyph_chars: &[char],
        atlas_width: u32,
        atlas_height: u32,
        pen_x: &mut u32,
        pen_y: &mut u32,
        row_h: &mut u32,
        max_font_y: u32,
        face_label: &str,
    ) -> Self {
        let line_metrics = font
            .horizontal_line_metrics(RASTER_PX)
            .expect("font lacks horizontal line metrics");
        let ascent = line_metrics.ascent;
        let line_height =
            (line_metrics.ascent - line_metrics.descent + line_metrics.line_gap).ceil();
        let effective_ascent = ascent * RASTER_PX / line_height.max(1.0);

        let mut glyphs = HashMap::with_capacity(glyph_chars.len());
        let mut rasters = Vec::with_capacity(glyph_chars.len());
        let requested = glyph_chars.len();

        for &ch in glyph_chars {
            let code = ch as u32;
            let (metrics, bitmap) = font.rasterize(ch, RASTER_PX);
            let gw = metrics.width as u32;
            let gh = metrics.height as u32;

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

            if *pen_x + gw + GLYPH_PAD > atlas_width {
                *pen_x = GLYPH_PAD;
                *pen_y += *row_h + GLYPH_PAD;
                *row_h = 0;
            }
            if *pen_y + gh + GLYPH_PAD > max_font_y {
                log::warn!(
                    "font ({face_label}): ran out of atlas room at U+{code:04X} '{}' \
                     (font region height {max_font_y}px, glyph would land at y={})",
                    ch,
                    *pen_y + gh,
                );
                break;
            }

            let u0 = *pen_x as f32 / atlas_width as f32;
            let v0 = *pen_y as f32 / atlas_height as f32;
            let u1 = (*pen_x + gw) as f32 / atlas_width as f32;
            let v1 = (*pen_y + gh) as f32 / atlas_height as f32;

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
            rasters.push((*pen_x, *pen_y, gw, gh, bitmap));

            *pen_x += gw + GLYPH_PAD;
            *row_h = (*row_h).max(gh);
        }

        if glyphs.len() < requested {
            log::warn!(
                "font ({face_label}): packed {}/{} glyphs (font region ends at y={})",
                glyphs.len(),
                requested,
                *pen_y + *row_h,
            );
        }

        Self {
            glyphs,
            rasters,
            line_height,
            ascent,
        }
    }

    fn glyph(&self, ch: char) -> Option<GlyphInfo> {
        self.glyphs.get(&ch).copied()
    }

    fn advance(&self, ch: char) -> f32 {
        self.glyphs
            .get(&ch)
            .map(|g| g.advance)
            .unwrap_or(RASTER_PX * 0.5)
    }
}

/// TTF-backed overlay atlas (body + header). Built once at startup.
pub struct BitmapFont {
    pub raster_px: f32,
    pub atlas_width: u32,
    pub atlas_height: u32,
    body: PackedFace,
    header: PackedFace,
}

impl BitmapFont {
    pub fn new() -> Self {
        Self::with_atlas_size(OVERLAY_ATLAS_SIZE, OVERLAY_ATLAS_SIZE.max(ICON_REGION_Y))
    }

    pub fn with_atlas_size(atlas_width: u32, atlas_height: u32) -> Self {
        let body_ttf =
            Font::from_bytes(BODY_FONT_BYTES, FontSettings::default()).expect("PT Serif TTF");
        let header_ttf =
            Font::from_bytes(HEADER_FONT_BYTES, FontSettings::default()).expect("Share Tech TTF");

        let glyph_chars: Vec<char> = (32u32..=126)
            .chain(160u32..=255)
            .filter_map(char::from_u32)
            .collect();

        let mut pen_x: u32 = 4 + GLYPH_PAD;
        let mut pen_y: u32 = GLYPH_PAD;
        let mut row_h: u32 = 0;

        let body = PackedFace::pack(
            &body_ttf,
            &glyph_chars,
            atlas_width,
            atlas_height,
            &mut pen_x,
            &mut pen_y,
            &mut row_h,
            ICON_REGION_Y,
            "body",
        );

        // Continue packing on the next shelf so header glyphs never
        // overlap body UVs.
        pen_x = pen_x.max(GLYPH_PAD);
        if pen_x > GLYPH_PAD {
            pen_x = GLYPH_PAD;
            pen_y += row_h + GLYPH_PAD;
            row_h = 0;
        }

        let header = PackedFace::pack(
            &header_ttf,
            &glyph_chars,
            atlas_width,
            atlas_height,
            &mut pen_x,
            &mut pen_y,
            &mut row_h,
            ICON_REGION_Y,
            "header",
        );

        Self {
            raster_px: RASTER_PX,
            atlas_width,
            atlas_height,
            body,
            header,
        }
    }

    #[inline]
    pub fn glyph_for(&self, header_face: bool, ch: char) -> Option<GlyphInfo> {
        if header_face {
            self.header.glyph(ch)
        } else {
            self.body.glyph(ch)
        }
    }

    /// Body face — backward compatible alias for call sites that
    /// only care about PT Serif.
    pub fn glyph(&self, ch: char) -> Option<GlyphInfo> {
        self.glyph_for(false, ch)
    }

    pub fn advance_for(&self, header_face: bool, ch: char) -> f32 {
        if header_face {
            self.header.advance(ch)
        } else {
            self.body.advance(ch)
        }
    }

    pub fn advance(&self, ch: char) -> f32 {
        self.advance_for(false, ch)
    }

    pub fn atlas_data(&self) -> Vec<u8> {
        let w = self.atlas_width as usize;
        let h = self.atlas_height as usize;
        let mut data = vec![0u8; w * h * 4];

        for y in 0..4usize.min(h) {
            for x in 0..4usize.min(w) {
                let dst = (y * w + x) * 4;
                data[dst] = 255;
                data[dst + 1] = 255;
                data[dst + 2] = 255;
                data[dst + 3] = 255;
            }
        }

        for face in [&self.body, &self.header] {
            for (px, py, gw, gh, bitmap) in &face.rasters {
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
        }

        data
    }
}
