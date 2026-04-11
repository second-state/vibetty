//! Simple test program for screenshot functionality

use tiny_skia::{Color, Paint, Pixmap, Rect, Transform};
use image::{ImageBuffer, Rgba, RgbaImage};
use vt100::Parser;

struct Canvas {
    background: Pixmap,
}

impl Canvas {
    fn new(width: u32, height: u32) -> Option<Self> {
        let background = Pixmap::new(width, height)?;
        Some(Self { background })
    }

    fn fill(&mut self, color: [u8; 4]) {
        let color = Color::from_rgba8(color[0], color[1], color[2], color[3]);
        self.background.fill(color);
    }

    fn fill_rect(&mut self, x: i32, y: i32, width: u32, height: u32, color: [u8; 4]) {
        if let Some(rect) = Rect::from_xywh(x as f32, y as f32, width as f32, height as f32) {
            let mut paint = Paint::default();
            paint.set_color(Color::from_rgba8(color[0], color[1], color[2], color[3]));
            let _ = self.background.fill_rect(rect, &paint, Transform::identity(), None);
        }
    }

    fn draw_text_simple(&mut self, text: &str, x: i32, y: i32, color: [u8; 4]) {
        for (i, ch) in text.chars().enumerate() {
            let px_x = x + i as i32 * 8;
            let px_y = y;
            if !ch.is_whitespace() {
                self.fill_rect(px_x, px_y, 6, 10, color);
            }
        }
    }

    fn to_image(self) -> Option<RgbaImage> {
        RgbaImage::from_raw(
            self.background.width(),
            self.background.height(),
            self.background.data().to_vec(),
        )
    }

    fn width(&self) -> u32 {
        self.background.width()
    }

    fn height(&self) -> u32 {
        self.background.height()
    }
}

fn color_to_rgba(color: vt100::Color) -> [u8; 4] {
    match color {
        vt100::Color::Default => [229, 229, 229, 255],
        vt100::Color::Idx(0) => [0, 0, 0, 255],
        vt100::Color::Idx(1) => [194, 54, 33, 255],
        vt100::Color::Idx(2) => [37, 188, 36, 255],
        vt100::Color::Idx(3) => [173, 173, 39, 255],
        vt100::Color::Idx(4) => [73, 46, 225, 255],
        vt100::Color::Idx(5) => [211, 56, 211, 255],
        vt100::Color::Idx(6) => [51, 187, 200, 255],
        vt100::Color::Idx(7) => [203, 204, 205, 255],
        vt100::Color::Idx(i) => {
            if i >= 232 {
                let shade = ((i - 232) * 10 + 8) as u8;
                [shade, shade, shade, 255]
            } else {
                [200, 200, 200, 255]
            }
        }
        vt100::Color::Rgb(r, g, b) => [r, g, b, 255],
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create a simple terminal screen with colored text
    let mut parser = Parser::new(24, 80, 8096);

    // Add some test content with ANSI colors
    parser.process(b"\x1b[31mRed Text\x1b[0m\n");
    parser.process(b"\x1b[32mGreen Text\x1b[0m\n");
    parser.process(b"\x1b[33mYellow Text\x1b[0m\n");
    parser.process(b"\x1b[34mBlue Text\x1b[0m\n");
    parser.process(b"\x1b[1mBold Text\x1b[0m\n");
    parser.process(b"Normal text with some content\n");

    let screen = parser.screen();
    let (rows, cols) = screen.size();

    let char_width = 8u32;
    let char_height = 16u32;
    let padding = 16u32;
    let title_height = 32u32;

    let image_width = cols as u32 * char_width + padding * 2;
    let image_height = rows as u32 * char_height + title_height + padding * 2;

    let mut canvas = Canvas::new(image_width, image_height)
        .ok_or("Failed to create canvas")?;

    // Fill background
    canvas.fill([30, 30, 30, 255]);

    // Draw title bar
    canvas.fill_rect(0, 0, image_width, title_height, [40, 40, 45, 255]);
    canvas.fill_rect(0, 30, image_width, 2, [60, 60, 65, 255]);
    canvas.draw_text_simple("Test Terminal", padding as i32 + 8, 10, [220, 220, 220, 255]);

    // Draw terminal content
    for row in 0..rows {
        for col in 0..cols {
            if let Some(cell) = screen.cell(row, col) {
                let x = padding + col as u32 * char_width;
                let y = title_height + padding + row as u32 * char_height;

                // Draw background color
                let bg = cell.bgcolor();
                if bg != vt100::Color::Default {
                    let color = color_to_rgba(bg);
                    canvas.fill_rect(x as i32, y as i32, char_width, char_height, color);
                }

                // Draw text
                if cell.has_contents() && !cell.is_wide_continuation() {
                    let fg = cell.fgcolor();
                    let fg_color = color_to_rgba(fg);
                    canvas.draw_text_simple(
                        cell.contents(),
                        x as i32,
                        y as i32 + 2,
                        fg_color,
                    );
                }
            }
        }
    }

    // Save image
    let image = canvas.to_image().ok_or("Failed to create image")?;
    image.save("test_screenshot.png")?;
    println!("Screenshot saved to test_screenshot.png");
    println!("Image size: {}x{}", image.width(), image.height());

    Ok(())
}
