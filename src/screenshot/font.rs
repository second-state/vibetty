//! Font loading utilities

use ab_glyph::{FontArc, PxScale};
use std::sync::LazyLock;

/// Global cached font, parsed only once
static FONT: LazyLock<FontArc> = LazyLock::new(|| {
    FontArc::try_from_slice(include_bytes!("../../assets/SarasaMonoSC-Light.ttf"))
        .expect("Embedded font is valid")
});

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
            font: FONT.clone(),
            scale: PxScale::from(font_size),
        }
    }
}

/// Load font with specific size
pub fn load_font_with_size(size: f32) -> Result<FontData, String> {
    Ok(FontData::new(size))
}
