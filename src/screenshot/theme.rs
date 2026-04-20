//! Color theme for terminal rendering

/// Theme for terminal colors
#[derive(Clone, Debug)]
pub struct Theme {
    /// Standard ANSI 16 colors
    pub colors: [[u8; 4]; 16],
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            colors: [
                // Normal colors
                [0, 0, 0, 255],       // Black
                [194, 54, 33, 255],   // Red
                [37, 188, 36, 255],   // Green
                [173, 173, 39, 255],  // Yellow
                [73, 46, 225, 255],   // Blue
                [211, 56, 211, 255],  // Magenta
                [51, 187, 200, 255],  // Cyan
                [203, 204, 205, 255], // White
                // Bright colors
                [128, 128, 128, 255], // Bright Black (Gray)
                [255, 85, 85, 255],   // Bright Red
                [85, 255, 85, 255],   // Bright Green
                [255, 255, 85, 255],  // Bright Yellow
                [85, 85, 255, 255],   // Bright Blue
                [255, 85, 255, 255],  // Bright Magenta
                [85, 255, 255, 255],  // Bright Cyan
                [255, 255, 255, 255], // Bright White
            ],
        }
    }
}

impl Theme {
    /// Convert a vt100 Color to RGBA
    pub fn color_to_rgba(&self, color: vt100::Color) -> [u8; 4] {
        match color {
            vt100::Color::Default => [229, 229, 229, 255],
            vt100::Color::Idx(i) => {
                if i < 16 {
                    self.colors[i as usize]
                } else if i >= 232 {
                    // Grayscale
                    let shade = ((i - 232) * 10 + 8) as u8;
                    [shade, shade, shade, 255]
                } else {
                    // 216-color cube
                    let i = i - 16;
                    let r = ((i / 36) * 51) as u8;
                    let g = (((i / 6) % 6) * 51) as u8;
                    let b = ((i % 6) * 51) as u8;
                    [r, g, b, 255]
                }
            }
            vt100::Color::Rgb(r, g, b) => [r, g, b, 255],
        }
    }

    /// Get foreground color with bold attribute
    pub fn get_foreground(&self, color: vt100::Color, bold: bool) -> [u8; 4] {
        if bold {
            match color {
                vt100::Color::Default => self.colors[15], // bright white
                vt100::Color::Idx(i) if i < 8 => {
                    // Map normal color to bright variant (index + 8)
                    self.colors[i as usize + 8]
                }
                _ => self.color_to_rgba(color),
            }
        } else {
            self.color_to_rgba(color)
        }
    }

    /// Get background color
    pub fn get_background(&self, color: vt100::Color) -> [u8; 4] {
        self.color_to_rgba(color)
    }
}
