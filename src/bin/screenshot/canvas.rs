use image::{ImageBuffer, Rgba, RgbaImage};
use tiny_skia::{Color, Paint, Pixmap, Rect, Transform};

pub struct Canvas {
    background: Pixmap,
    text_layer: ImageBuffer<Rgba<u8>, Vec<u8>>,
}

impl Canvas {
    pub fn new(width: u32, height: u32) -> Result<Self, String> {
        let background = Pixmap::new(width, height)
            .ok_or_else(|| "Failed to create pixmap".to_string())?;
        let text_layer = ImageBuffer::new(width, height);
        Ok(Self { background, text_layer })
    }

    pub fn fill(&mut self, color: [u8; 4]) {
        let color = Color::from_rgba8(color[0], color[1], color[2], color[3]);
        self.background.fill(color);
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, width: u32, height: u32, color: [u8; 4]) {
        if let Some(rect) = Rect::from_xywh(x as f32, y as f32, width as f32, height as f32) {
            let mut paint = Paint::default();
            paint.set_color(Color::from_rgba8(color[0], color[1], color[2], color[3]));
            let _ = self.background.fill_rect(rect, &paint, Transform::identity(), None);
        }
    }

    pub fn draw_text_simple(&mut self, text: &str, x: i32, y: i32, color: [u8; 4]) {
        // Simple character rendering using rectangles
        for (i, ch) in text.chars().enumerate() {
            let px_x = x + i as i32 * 8;
            let px_y = y;
            if !ch.is_whitespace() {
                self.fill_rect(px_x, px_y, 6, 10, color);
            }
        }
    }

    pub fn draw_text_with_font(&mut self, text: &str, x: i32, y: i32, color: [u8; 4], font: &ab_glyph::FontArc, scale: ab_glyph::PxScale) {
        let rgba = image::Rgba(color);
        imageproc::drawing::draw_text_mut(&mut self.text_layer, rgba, x, y, scale, font, text);
    }

    pub fn to_image(self) -> Result<RgbaImage, String> {
        let mut final_image = RgbaImage::from_raw(
            self.background.width(),
            self.background.height(),
            self.background.data().to_vec(),
        )
        .ok_or_else(|| "Failed to create image".to_string())?;

        for (final_pixel, text_pixel) in final_image.pixels_mut().zip(self.text_layer.pixels()) {
            let alpha = text_pixel[3] as f32 / 255.0;
            if alpha > 0.0 {
                for i in 0..3 {
                    final_pixel[i] = (text_pixel[i] as f32 * alpha + final_pixel[i] as f32 * (1.0 - alpha)) as u8;
                }
                final_pixel[3] = 255;
            }
        }

        Ok(final_image)
    }
}
