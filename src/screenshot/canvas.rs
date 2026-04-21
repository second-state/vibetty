//! Canvas for rendering terminal content to images

use ab_glyph::{Font, PxScale};
use image::{ImageBuffer, Rgba, RgbaImage};
use imageproc::drawing::draw_text_mut;
use tiny_skia::{Color, Paint, Pixmap, Rect, Transform};

/// Canvas for drawing shapes and text
pub struct Canvas {
    background: Pixmap,
    text_layer: ImageBuffer<Rgba<u8>, Vec<u8>>,
    char_width: u32,
    char_height: u32,
}

impl Canvas {
    /// Create a new canvas with the given dimensions
    pub fn new(width: u32, height: u32) -> Result<Self, String> {
        let background =
            Pixmap::new(width, height).ok_or_else(|| "Failed to create pixmap".to_string())?;

        let text_layer = ImageBuffer::new(width, height);

        Ok(Self {
            background,
            text_layer,
            char_width: 8,
            char_height: 16,
        })
    }

    /// Set character size for text rendering
    pub fn set_char_size(&mut self, width: u32, height: u32) {
        self.char_width = width;
        self.char_height = height;
    }

    /// Get character width
    #[allow(dead_code)]
    pub fn char_width(&self) -> u32 {
        self.char_width
    }

    /// Get character height
    #[allow(dead_code)]
    pub fn char_height(&self) -> u32 {
        self.char_height
    }

    /// Fill the entire canvas with a color
    pub fn fill(&mut self, color: [u8; 4]) {
        let color = Color::from_rgba8(color[0], color[1], color[2], color[3]);
        self.background.fill(color);
    }

    /// Fill a rectangle with a color
    pub fn fill_rect(&mut self, x: i32, y: i32, width: u32, height: u32, color: [u8; 4]) {
        if let Some(rect) = Rect::from_xywh(x as f32, y as f32, width as f32, height as f32) {
            let mut paint = Paint::default();
            paint.set_color(Color::from_rgba8(color[0], color[1], color[2], color[3]));
            self.background
                .fill_rect(rect, &paint, Transform::identity(), None);
        }
    }

    /// Draw a title bar background at the top
    pub fn draw_title_bar(&mut self, _title: &str, _padding: u32) {
        let height = 32;
        let bg = [40, 40, 45, 255];
        self.fill_rect(0, 0, self.width(), height, bg);
        self.fill_rect(0, height as i32 - 2, self.width(), 2, [60, 60, 65, 255]);
    }

    /// Draw text at the specified position (simple placeholder)
    #[allow(dead_code)]
    pub fn draw_text(
        &mut self,
        text: &str,
        x: i32,
        y: i32,
        color: [u8; 4],
        _font: Option<&()>,
        _font_size: f32,
    ) {
        self.draw_text_simple(text, x, y, color);
    }

    /// Draw text at the specified position (simple version without font)
    #[allow(dead_code)]
    pub fn draw_text_simple(&mut self, text: &str, x: i32, y: i32, color: [u8; 4]) {
        for (i, ch) in text.chars().enumerate() {
            let px_x = x + i as i32 * 8;
            let px_y = y;
            if !ch.is_whitespace() {
                self.fill_rect(px_x, px_y, 6, 10, color);
            }
        }
    }

    /// Draw text using ab_glyph font
    pub fn draw_text_with_font<F: Font>(
        &mut self,
        text: &str,
        x: i32,
        y: i32,
        color: [u8; 4],
        font: &F,
        scale: PxScale,
    ) {
        let rgba = Rgba(color);
        draw_text_mut(&mut self.text_layer, rgba, x, y, scale, font, text);
    }

    /// Get the canvas width
    pub fn width(&self) -> u32 {
        self.background.width()
    }

    /// Get the canvas height
    #[allow(dead_code)]
    pub fn height(&self) -> u32 {
        self.background.height()
    }

    /// Convert the canvas to a final image
    pub fn into_image(self) -> Result<RgbaImage, String> {
        let mut final_image = RgbaImage::from_raw(
            self.background.width(),
            self.background.height(),
            self.background.data().to_vec(),
        )
        .ok_or_else(|| "Failed to create image from raw data".to_string())?;

        // Blend text layer on top (fast integer alpha blending)
        for (dst, src) in final_image.pixels_mut().zip(self.text_layer.pixels()) {
            let a = src[3] as u32;
            if a == 0 {
                continue;
            }
            if a == 255 {
                dst[0] = src[0];
                dst[1] = src[1];
                dst[2] = src[2];
                dst[3] = 255;
            } else {
                let inv_a = 255 - a;
                dst[0] = ((src[0] as u32 * a + dst[0] as u32 * inv_a + 128) >> 8) as u8;
                dst[1] = ((src[1] as u32 * a + dst[1] as u32 * inv_a + 128) >> 8) as u8;
                dst[2] = ((src[2] as u32 * a + dst[2] as u32 * inv_a + 128) >> 8) as u8;
                dst[3] = 255;
            }
        }

        Ok(final_image)
    }
}
