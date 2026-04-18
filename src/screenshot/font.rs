//! Font loading utilities

use ab_glyph::{FontArc, PxScale};

/// Font data container with ab_glyph FontArc
pub struct FontData {
    /// Regular font
    pub font: FontArc,
    /// Font scale for rendering
    pub scale: PxScale,
}

impl FontData {
    /// Create a new FontData with the given font size
    pub fn new(font_size: f32) -> Self {
        Self {
            font: Self::load_font_arc(),
            scale: PxScale::from(font_size),
        }
    }

    /// Load font: try system fonts first, fall back to embedded font
    fn load_font_arc() -> FontArc {
        // Try system monospace fonts first
        let system_paths = [
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
            "/usr/share/fonts/truetype/ubuntu/UbuntuMono-R.ttf",
            "/usr/share/fonts/truetype/freefont/FreeMono.ttf",
            "/System/Library/Fonts/Monaco.ttc",
        ];

        for path in &system_paths {
            if std::path::Path::new(path).exists() {
                if let Ok(data) = std::fs::read(path) {
                    if let Ok(font) = FontArc::try_from_vec(data) {
                        log::info!("Loaded system font from: {}", path);
                        return font;
                    }
                }
            }
        }

        // Fall back to embedded font
        log::info!("Using embedded font");
        FontArc::try_from_slice(include_bytes!("../../assets/FiraMono-Regular.ttf"))
            .expect("Embedded font is valid")
    }
}

/// Load font with specific size
pub fn load_font_with_size(size: f32) -> Result<FontData, String> {
    Ok(FontData::new(size))
}
