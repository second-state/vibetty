use ab_glyph::{Font, FontArc, PxScale};

pub struct FontData {
    pub font: FontArc,
    pub scale: PxScale,
}

impl FontData {
    pub fn new(font_size: f32) -> Result<Self, Box<dyn std::error::Error>> {
        let font_data = Self::load_system_font()
            .ok_or("Font not found")?;

        let font_data_static: &'static [u8] = Box::leak(font_data.into_boxed_slice());
        let font = FontArc::try_from_slice(font_data_static)
            .map_err(|_| "Failed to load font".to_string())?;

        Ok(Self {
            font,
            scale: PxScale::from(font_size),
        })
    }

    fn load_system_font() -> Option<Vec<u8>> {
        let paths = [
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
            "/usr/share/fonts/truetype/ubuntu/UbuntuMono-R.ttf",
        ];

        for path in &paths {
            if std::path::Path::new(path).exists() {
                if let Ok(data) = std::fs::read(path) {
                    log::info!("Loaded font from: {}", path);
                    return Some(data);
                }
            }
        }
        None
    }

    pub fn with_fallback(font_size: f32) -> Self {
        let font = Self::load_system_font()
            .and_then(|data| {
                let data_static: &'static [u8] = Box::leak(data.into_boxed_slice());
                FontArc::try_from_slice(data_static).ok()
            })
            .unwrap_or_else(|| {
                log::warn!("No system font found");
                Self::create_latin1_font()
            });

        Self {
            font,
            scale: PxScale::from(font_size),
        }
    }

    fn create_latin1_font() -> FontArc {
        // Try to find any TTF font
        let paths = [
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/freefont/FreeSans.ttf",
        ];

        for path in &paths {
            if std::path::Path::new(path).exists() {
                if let Ok(data) = std::fs::read(path) {
                    let data_static: &'static [u8] = Box::leak(data.into_boxed_slice());
                    if let Ok(font) = FontArc::try_from_slice(data_static) {
                        return font;
                    }
                }
            }
        }

        panic!("No font found. Please install a monospace font.");
    }
}
