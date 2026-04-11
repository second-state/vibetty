//! Font loading utilities

use ab_glyph::{Font, FontArc, PxScale};
use std::path::Path;

#[derive(thiserror::Error, Debug)]
pub enum FontError {
    #[error("Font not found")]
    NotFound,
    #[error("Failed to load font data")]
    LoadFailed,
}

/// Font data container with ab_glyph FontArc
pub struct FontData {
    /// Regular font
    pub font: FontArc,
    /// Font scale for rendering
    pub scale: PxScale,
}

impl FontData {
    /// Create a new FontData with the given font size
    pub fn new(font_size: f32) -> Result<Self, FontError> {
        let font = Self::load_font_arc().ok_or(FontError::NotFound)?;
        Ok(Self {
            font,
            scale: PxScale::from(font_size),
        })
    }

    /// Load FontArc from system fonts
    fn load_font_arc() -> Option<FontArc> {
        // Common monospace font paths
        let paths = [
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
            "/usr/share/fonts/truetype/ubuntu/UbuntuMono-R.ttf",
            "/usr/share/fonts/truetype/freefont/FreeMono.ttf",
            "/System/Library/Fonts/Monaco.ttc",
            "C:\\Windows\\Fonts\\consola.ttf",
        ];

        for path in &paths {
            if Path::new(path).exists() {
                if let Ok(data) = std::fs::read(path) {
                    log::info!("Loaded font from: {}", path);
                    // Leak the data to get 'static lifetime
                    let data_static: &'static [u8] = Box::leak(data.into_boxed_slice());
                    if let Ok(font) = FontArc::try_from_slice(data_static) {
                        return Some(font);
                    }
                }
            }
        }
        None
    }

    /// Create with built-in fallback font
    pub fn with_fallback(font_size: f32) -> Self {
        let font = Self::load_font_arc()
            .unwrap_or_else(|| {
                // Fallback: try to use ab_glyph's Latin1
                // Since we can't easily get DEFAULT, we'll use a minimal approach
                log::warn!("Using fallback: no system font found, text rendering may be limited");
                // Create a dummy font that won't crash but won't render properly
                // In production, you'd embed a font file
                Self::create_dummy_font()
            });

        Self {
            font,
            scale: PxScale::from(font_size),
        }
    }

    /// Create a dummy font for fallback (not ideal, but prevents crashes)
    fn create_dummy_font() -> FontArc {
        // In a real application, you would embed a font file here
        // For now, we'll try to find any TTF file on the system
        let font = Self::find_any_ttf()
            .and_then(|data| {
                let data_static: &'static [u8] = Box::leak(data.into_boxed_slice());
                FontArc::try_from_slice(data_static).ok()
            })
            .unwrap_or_else(|| {
                // If absolutely nothing works, we have a problem
                // In production, embed a small font file using include_bytes!
                panic!("No font found. Please install a monospace font (DejaVu Sans Mono, Liberation Mono, etc.)");
            });

        font
    }

    /// Find any TTF font on the system
    fn find_any_ttf() -> Option<Vec<u8>> {
        let paths = [
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/freefont/FreeSans.ttf",
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
        ];

        for path in &paths {
            if Path::new(path).exists() {
                if let Ok(data) = std::fs::read(path) {
                    log::info!("Using fallback font: {}", path);
                    return Some(data);
                }
            }
        }
        None
    }

    /// Check if a system font is available
    pub fn is_font_available() -> bool {
        Self::load_font_arc().is_some()
    }
}

/// Public function to load font for screenshot module
pub fn load_font() -> Result<FontData, FontError> {
    FontData::new(14.0)
}

/// Public function to load font with specific size
pub fn load_font_with_size(size: f32) -> Result<FontData, FontError> {
    FontData::new(size)
}
