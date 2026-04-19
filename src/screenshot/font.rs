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
        FontArc::try_from_slice(include_bytes!("../../assets/SarasaMonoSC-Light.ttf"))
            .expect("Embedded font is valid")
    }
}

/// Load font with specific size
pub fn load_font_with_size(size: f32) -> Result<FontData, String> {
    Ok(FontData::new(size))
}
