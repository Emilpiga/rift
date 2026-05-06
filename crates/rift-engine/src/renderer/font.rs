/// Bitmap font using a simple 6x8 monospace pixel font.
///
/// The font lives in the top-left corner of a larger 256x256 RGBA
/// overlay atlas \u2014 the rest of that atlas is reserved for icon
/// glyphs (ability icons, item icons, ...) loaded by the renderer
/// at startup. Glyph UVs are computed against the *full* atlas
/// dimensions so callers can sample directly without scaling.
///
/// First pixel (0,0) is guaranteed solid white opaque so any
/// vertex that wants a flat colour rect can use UV = (0,0).

/// Side length of the combined overlay atlas in pixels.
pub const OVERLAY_ATLAS_SIZE: u32 = 256;
/// y-coordinate where the icon region starts. The font region is
/// `0..ICON_REGION_Y` along the vertical axis. Kept aligned to a
/// power-of-two so an icon row stays inside the texture cleanly.
pub const ICON_REGION_Y: u32 = 64;

pub struct GlyphInfo {
    pub u0: f32,
    pub v0: f32,
    pub u1: f32,
    pub v1: f32,
}

pub struct BitmapFont {
    pub glyph_width: u32,
    pub glyph_height: u32,
    /// Full overlay atlas dimensions (font + icon region).
    pub atlas_width: u32,
    pub atlas_height: u32,
    cols: u32,
}

impl BitmapFont {
    pub fn new() -> Self {
        Self::with_atlas_size(OVERLAY_ATLAS_SIZE, OVERLAY_ATLAS_SIZE)
    }

    /// Construct a font that paints into an atlas of the given
    /// dimensions. The font region itself is fixed-size (top-left
    /// 97x48); growing the atlas just creates more room for the
    /// icon region. Width must be at least the font's native
    /// width and height must be at least `ICON_REGION_Y`.
    pub fn with_atlas_size(atlas_width: u32, atlas_height: u32) -> Self {
        // 6x8 font, 16 columns x 6 rows = 96 glyphs (ASCII 32..127).
        // Glyphs occupy the top-left 97x48 region.
        Self {
            glyph_width: 6,
            glyph_height: 8,
            atlas_width,
            atlas_height,
            cols: 16,
        }
    }

    /// Get glyph UV coordinates for a character.
    pub fn glyph(&self, ch: char) -> Option<GlyphInfo> {
        let code = ch as u32;
        if code < 32 || code > 126 {
            return None;
        }
        let index = code - 32;
        let col = index % self.cols;
        let row = index / self.cols;

        // +1 pixel offset for the white column at x=0
        let px_x = 1 + col * self.glyph_width;
        let px_y = row * self.glyph_height;

        let u0 = px_x as f32 / self.atlas_width as f32;
        let v0 = px_y as f32 / self.atlas_height as f32;
        let u1 = (px_x + self.glyph_width) as f32 / self.atlas_width as f32;
        let v1 = (px_y + self.glyph_height) as f32 / self.atlas_height as f32;

        Some(GlyphInfo { u0, v0, u1, v1 })
    }

    /// Generate the combined overlay atlas pixel data as RGBA8.
    /// The font region (top-left) stores glyph masks as
    /// `(255, 255, 255, mask)`; the icon region (`y >= ICON_REGION_Y`)
    /// is left zeroed out for the renderer to fill at startup.
    pub fn atlas_data(&self) -> Vec<u8> {
        let w = self.atlas_width as usize;
        let h = self.atlas_height as usize;
        let mut data = vec![0u8; w * h * 4];

        // Column 0 of the font region: solid white opaque pixels
        // \u2014 vertices that want a flat colour rect sample here.
        let font_h = (self.glyph_height * 6) as usize; // 48
        for y in 0..font_h {
            let dst = y * w * 4;
            data[dst] = 255;
            data[dst + 1] = 255;
            data[dst + 2] = 255;
            data[dst + 3] = 255;
        }

        // Render each glyph as white with alpha = mask bit.
        for code in 32u32..=126 {
            let index = code - 32;
            let col = (index % self.cols) as usize;
            let row = (index / self.cols) as usize;
            let base_x = 1 + col * self.glyph_width as usize;
            let base_y = row * self.glyph_height as usize;

            let glyph_data = get_glyph_bitmap(code as u8);
            for gy in 0..8usize {
                let row_bits = glyph_data[gy];
                for gx in 0..6usize {
                    if (row_bits >> (5 - gx)) & 1 != 0 {
                        let px = base_x + gx;
                        let py = base_y + gy;
                        if px < w && py < h {
                            let dst = (py * w + px) * 4;
                            data[dst] = 255;
                            data[dst + 1] = 255;
                            data[dst + 2] = 255;
                            data[dst + 3] = 255;
                        }
                    }
                }
            }
        }

        data
    }
}

/// Get 6x8 bitmap for an ASCII character (rows top to bottom, 6 bits per row MSB-first).
fn get_glyph_bitmap(ch: u8) -> [u8; 8] {
    if ch < 32 || ch > 126 {
        return [0; 8];
    }
    FONT_DATA[(ch - 32) as usize]
}

/// 6x8 pixel font data for ASCII 32-126 (95 glyphs).
/// Each glyph is 8 bytes (rows), each row has 6 pixels in the upper 6 bits.
#[rustfmt::skip]
const FONT_DATA: [[u8; 8]; 95] = [
    // 32 ' ' (space)
    [0b000000, 0b000000, 0b000000, 0b000000, 0b000000, 0b000000, 0b000000, 0b000000],
    // 33 '!'
    [0b001000, 0b001000, 0b001000, 0b001000, 0b001000, 0b000000, 0b001000, 0b000000],
    // 34 '"'
    [0b010100, 0b010100, 0b010100, 0b000000, 0b000000, 0b000000, 0b000000, 0b000000],
    // 35 '#'
    [0b010100, 0b010100, 0b111110, 0b010100, 0b111110, 0b010100, 0b010100, 0b000000],
    // 36 '$'
    [0b001000, 0b011110, 0b101000, 0b011100, 0b001010, 0b111100, 0b001000, 0b000000],
    // 37 '%'
    [0b110000, 0b110010, 0b000100, 0b001000, 0b010000, 0b100110, 0b000110, 0b000000],
    // 38 '&'
    [0b011000, 0b100100, 0b101000, 0b010000, 0b101010, 0b100100, 0b011010, 0b000000],
    // 39 '''
    [0b001000, 0b001000, 0b010000, 0b000000, 0b000000, 0b000000, 0b000000, 0b000000],
    // 40 '('
    [0b000100, 0b001000, 0b010000, 0b010000, 0b010000, 0b001000, 0b000100, 0b000000],
    // 41 ')'
    [0b100000, 0b010000, 0b001000, 0b001000, 0b001000, 0b010000, 0b100000, 0b000000],
    // 42 '*'
    [0b000000, 0b001000, 0b101010, 0b011100, 0b101010, 0b001000, 0b000000, 0b000000],
    // 43 '+'
    [0b000000, 0b001000, 0b001000, 0b111110, 0b001000, 0b001000, 0b000000, 0b000000],
    // 44 ','
    [0b000000, 0b000000, 0b000000, 0b000000, 0b000000, 0b001000, 0b001000, 0b010000],
    // 45 '-'
    [0b000000, 0b000000, 0b000000, 0b111110, 0b000000, 0b000000, 0b000000, 0b000000],
    // 46 '.'
    [0b000000, 0b000000, 0b000000, 0b000000, 0b000000, 0b000000, 0b001000, 0b000000],
    // 47 '/'
    [0b000000, 0b000010, 0b000100, 0b001000, 0b010000, 0b100000, 0b000000, 0b000000],
    // 48 '0'
    [0b011100, 0b100010, 0b100110, 0b101010, 0b110010, 0b100010, 0b011100, 0b000000],
    // 49 '1'
    [0b001000, 0b011000, 0b001000, 0b001000, 0b001000, 0b001000, 0b011100, 0b000000],
    // 50 '2'
    [0b011100, 0b100010, 0b000010, 0b001100, 0b010000, 0b100000, 0b111110, 0b000000],
    // 51 '3'
    [0b011100, 0b100010, 0b000010, 0b001100, 0b000010, 0b100010, 0b011100, 0b000000],
    // 52 '4'
    [0b000100, 0b001100, 0b010100, 0b100100, 0b111110, 0b000100, 0b000100, 0b000000],
    // 53 '5'
    [0b111110, 0b100000, 0b111100, 0b000010, 0b000010, 0b100010, 0b011100, 0b000000],
    // 54 '6'
    [0b011100, 0b100000, 0b100000, 0b111100, 0b100010, 0b100010, 0b011100, 0b000000],
    // 55 '7'
    [0b111110, 0b000010, 0b000100, 0b001000, 0b010000, 0b010000, 0b010000, 0b000000],
    // 56 '8'
    [0b011100, 0b100010, 0b100010, 0b011100, 0b100010, 0b100010, 0b011100, 0b000000],
    // 57 '9'
    [0b011100, 0b100010, 0b100010, 0b011110, 0b000010, 0b000010, 0b011100, 0b000000],
    // 58 ':'
    [0b000000, 0b000000, 0b001000, 0b000000, 0b000000, 0b001000, 0b000000, 0b000000],
    // 59 ';'
    [0b000000, 0b000000, 0b001000, 0b000000, 0b000000, 0b001000, 0b001000, 0b010000],
    // 60 '<'
    [0b000100, 0b001000, 0b010000, 0b100000, 0b010000, 0b001000, 0b000100, 0b000000],
    // 61 '='
    [0b000000, 0b000000, 0b111110, 0b000000, 0b111110, 0b000000, 0b000000, 0b000000],
    // 62 '>'
    [0b100000, 0b010000, 0b001000, 0b000100, 0b001000, 0b010000, 0b100000, 0b000000],
    // 63 '?'
    [0b011100, 0b100010, 0b000010, 0b000100, 0b001000, 0b000000, 0b001000, 0b000000],
    // 64 '@'
    [0b011100, 0b100010, 0b101110, 0b101010, 0b101110, 0b100000, 0b011100, 0b000000],
    // 65 'A'
    [0b011100, 0b100010, 0b100010, 0b111110, 0b100010, 0b100010, 0b100010, 0b000000],
    // 66 'B'
    [0b111100, 0b100010, 0b100010, 0b111100, 0b100010, 0b100010, 0b111100, 0b000000],
    // 67 'C'
    [0b011100, 0b100010, 0b100000, 0b100000, 0b100000, 0b100010, 0b011100, 0b000000],
    // 68 'D'
    [0b111100, 0b100010, 0b100010, 0b100010, 0b100010, 0b100010, 0b111100, 0b000000],
    // 69 'E'
    [0b111110, 0b100000, 0b100000, 0b111100, 0b100000, 0b100000, 0b111110, 0b000000],
    // 70 'F'
    [0b111110, 0b100000, 0b100000, 0b111100, 0b100000, 0b100000, 0b100000, 0b000000],
    // 71 'G'
    [0b011100, 0b100010, 0b100000, 0b101110, 0b100010, 0b100010, 0b011100, 0b000000],
    // 72 'H'
    [0b100010, 0b100010, 0b100010, 0b111110, 0b100010, 0b100010, 0b100010, 0b000000],
    // 73 'I'
    [0b011100, 0b001000, 0b001000, 0b001000, 0b001000, 0b001000, 0b011100, 0b000000],
    // 74 'J'
    [0b000010, 0b000010, 0b000010, 0b000010, 0b000010, 0b100010, 0b011100, 0b000000],
    // 75 'K'
    [0b100010, 0b100100, 0b101000, 0b110000, 0b101000, 0b100100, 0b100010, 0b000000],
    // 76 'L'
    [0b100000, 0b100000, 0b100000, 0b100000, 0b100000, 0b100000, 0b111110, 0b000000],
    // 77 'M'
    [0b100010, 0b110110, 0b101010, 0b101010, 0b100010, 0b100010, 0b100010, 0b000000],
    // 78 'N'
    [0b100010, 0b110010, 0b101010, 0b100110, 0b100010, 0b100010, 0b100010, 0b000000],
    // 79 'O'
    [0b011100, 0b100010, 0b100010, 0b100010, 0b100010, 0b100010, 0b011100, 0b000000],
    // 80 'P'
    [0b111100, 0b100010, 0b100010, 0b111100, 0b100000, 0b100000, 0b100000, 0b000000],
    // 81 'Q'
    [0b011100, 0b100010, 0b100010, 0b100010, 0b101010, 0b100100, 0b011010, 0b000000],
    // 82 'R'
    [0b111100, 0b100010, 0b100010, 0b111100, 0b101000, 0b100100, 0b100010, 0b000000],
    // 83 'S'
    [0b011100, 0b100010, 0b100000, 0b011100, 0b000010, 0b100010, 0b011100, 0b000000],
    // 84 'T'
    [0b111110, 0b001000, 0b001000, 0b001000, 0b001000, 0b001000, 0b001000, 0b000000],
    // 85 'U'
    [0b100010, 0b100010, 0b100010, 0b100010, 0b100010, 0b100010, 0b011100, 0b000000],
    // 86 'V'
    [0b100010, 0b100010, 0b100010, 0b100010, 0b010100, 0b010100, 0b001000, 0b000000],
    // 87 'W'
    [0b100010, 0b100010, 0b100010, 0b101010, 0b101010, 0b110110, 0b100010, 0b000000],
    // 88 'X'
    [0b100010, 0b100010, 0b010100, 0b001000, 0b010100, 0b100010, 0b100010, 0b000000],
    // 89 'Y'
    [0b100010, 0b100010, 0b010100, 0b001000, 0b001000, 0b001000, 0b001000, 0b000000],
    // 90 'Z'
    [0b111110, 0b000010, 0b000100, 0b001000, 0b010000, 0b100000, 0b111110, 0b000000],
    // 91 '['
    [0b011100, 0b010000, 0b010000, 0b010000, 0b010000, 0b010000, 0b011100, 0b000000],
    // 92 '\'
    [0b000000, 0b100000, 0b010000, 0b001000, 0b000100, 0b000010, 0b000000, 0b000000],
    // 93 ']'
    [0b011100, 0b000100, 0b000100, 0b000100, 0b000100, 0b000100, 0b011100, 0b000000],
    // 94 '^'
    [0b001000, 0b010100, 0b100010, 0b000000, 0b000000, 0b000000, 0b000000, 0b000000],
    // 95 '_'
    [0b000000, 0b000000, 0b000000, 0b000000, 0b000000, 0b000000, 0b111110, 0b000000],
    // 96 '`'
    [0b010000, 0b001000, 0b000100, 0b000000, 0b000000, 0b000000, 0b000000, 0b000000],
    // 97 'a'
    [0b000000, 0b000000, 0b011100, 0b000010, 0b011110, 0b100010, 0b011110, 0b000000],
    // 98 'b'
    [0b100000, 0b100000, 0b111100, 0b100010, 0b100010, 0b100010, 0b111100, 0b000000],
    // 99 'c'
    [0b000000, 0b000000, 0b011100, 0b100000, 0b100000, 0b100000, 0b011100, 0b000000],
    // 100 'd'
    [0b000010, 0b000010, 0b011110, 0b100010, 0b100010, 0b100010, 0b011110, 0b000000],
    // 101 'e'
    [0b000000, 0b000000, 0b011100, 0b100010, 0b111110, 0b100000, 0b011100, 0b000000],
    // 102 'f'
    [0b001100, 0b010000, 0b010000, 0b111000, 0b010000, 0b010000, 0b010000, 0b000000],
    // 103 'g'
    [0b000000, 0b000000, 0b011110, 0b100010, 0b100010, 0b011110, 0b000010, 0b011100],
    // 104 'h'
    [0b100000, 0b100000, 0b111100, 0b100010, 0b100010, 0b100010, 0b100010, 0b000000],
    // 105 'i'
    [0b001000, 0b000000, 0b011000, 0b001000, 0b001000, 0b001000, 0b011100, 0b000000],
    // 106 'j'
    [0b000100, 0b000000, 0b000100, 0b000100, 0b000100, 0b000100, 0b100100, 0b011000],
    // 107 'k'
    [0b100000, 0b100000, 0b100100, 0b101000, 0b110000, 0b101000, 0b100100, 0b000000],
    // 108 'l'
    [0b011000, 0b001000, 0b001000, 0b001000, 0b001000, 0b001000, 0b011100, 0b000000],
    // 109 'm'
    [0b000000, 0b000000, 0b110100, 0b101010, 0b101010, 0b101010, 0b101010, 0b000000],
    // 110 'n'
    [0b000000, 0b000000, 0b111100, 0b100010, 0b100010, 0b100010, 0b100010, 0b000000],
    // 111 'o'
    [0b000000, 0b000000, 0b011100, 0b100010, 0b100010, 0b100010, 0b011100, 0b000000],
    // 112 'p'
    [0b000000, 0b000000, 0b111100, 0b100010, 0b100010, 0b111100, 0b100000, 0b100000],
    // 113 'q'
    [0b000000, 0b000000, 0b011110, 0b100010, 0b100010, 0b011110, 0b000010, 0b000010],
    // 114 'r'
    [0b000000, 0b000000, 0b101100, 0b110000, 0b100000, 0b100000, 0b100000, 0b000000],
    // 115 's'
    [0b000000, 0b000000, 0b011100, 0b100000, 0b011100, 0b000010, 0b111100, 0b000000],
    // 116 't'
    [0b010000, 0b010000, 0b111000, 0b010000, 0b010000, 0b010000, 0b001100, 0b000000],
    // 117 'u'
    [0b000000, 0b000000, 0b100010, 0b100010, 0b100010, 0b100010, 0b011110, 0b000000],
    // 118 'v'
    [0b000000, 0b000000, 0b100010, 0b100010, 0b100010, 0b010100, 0b001000, 0b000000],
    // 119 'w'
    [0b000000, 0b000000, 0b100010, 0b100010, 0b101010, 0b101010, 0b010100, 0b000000],
    // 120 'x'
    [0b000000, 0b000000, 0b100010, 0b010100, 0b001000, 0b010100, 0b100010, 0b000000],
    // 121 'y'
    [0b000000, 0b000000, 0b100010, 0b100010, 0b100010, 0b011110, 0b000010, 0b011100],
    // 122 'z'
    [0b000000, 0b000000, 0b111110, 0b000100, 0b001000, 0b010000, 0b111110, 0b000000],
    // 123 '{'
    [0b000100, 0b001000, 0b001000, 0b010000, 0b001000, 0b001000, 0b000100, 0b000000],
    // 124 '|'
    [0b001000, 0b001000, 0b001000, 0b001000, 0b001000, 0b001000, 0b001000, 0b000000],
    // 125 '}'
    [0b100000, 0b010000, 0b010000, 0b001000, 0b010000, 0b010000, 0b100000, 0b000000],
    // 126 '~'
    [0b000000, 0b010000, 0b101010, 0b000100, 0b000000, 0b000000, 0b000000, 0b000000],
];
