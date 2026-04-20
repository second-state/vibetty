//! Terminal screenshot functionality
//!
//! Renders a vt100::Screen to an image file.

mod canvas;
mod font;
mod theme;
mod utils;

pub use canvas::Canvas;
pub use font::{FontData, load_font_with_size};
pub use theme::Theme;

use ab_glyph::Font;
use image::ImageError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ScreenshotError {
    #[error("Failed to load font: {0}")]
    FontLoadError(String),

    #[error("Canvas error: {0}")]
    CanvasError(String),

    #[error("Image error: {0}")]
    ImageError(#[from] ImageError),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

/// Configuration for screenshot generation
pub struct ScreenshotConfig {
    /// Font size in points
    pub font_size: f32,

    /// Padding around the content (in pixels)
    pub padding: u32,

    /// Background color (R, G, B, A)
    pub background_color: [u8; 4],

    /// Whether to show window decorations
    pub show_decorations: bool,

    /// Window title
    pub title: Option<String>,
}

impl Default for ScreenshotConfig {
    fn default() -> Self {
        Self {
            font_size: 14.0,
            padding: 16,
            background_color: [30, 30, 30, 255],
            show_decorations: true,
            title: None,
        }
    }
}

/// Capture a vt100::Screen as an image
pub fn capture_screen(
    screen: &vt100::Screen,
    config: &ScreenshotConfig,
) -> Result<image::RgbaImage, ScreenshotError> {
    // Try to load font, fallback to built-in if not available
    let font_data =
        load_font_with_size(config.font_size).unwrap_or_else(|_| FontData::new(config.font_size));

    let theme = Theme::default();

    // Calculate character dimensions from the font
    let units_per_em = font_data.font.units_per_em().unwrap_or(2048.0);
    let space_id = font_data.font.glyph_id(' ');
    let advance_unscaled = font_data.font.h_advance_unscaled(space_id);
    let char_width = ((advance_unscaled / units_per_em) * font_data.scale.x).round() as u32;

    // Use proper line height = ascent + descent
    let ascent =
        (font_data.font.ascent_unscaled() / units_per_em * font_data.scale.y).round() as u32;
    let descent =
        (font_data.font.descent_unscaled() / units_per_em * font_data.scale.y).round() as u32;
    let char_height = ascent + descent;

    let (rows, cols) = screen.size();
    let padding = config.padding;
    let title_height = if config.show_decorations { 32 } else { 0 };

    // Find the last row with actual content (skip trailing empty rows)
    let mut last_content_row = 0;
    'b: for row in (0..rows).rev() {
        for col in 0..cols {
            if let Some(cell) = screen.cell(row, col) {
                if cell.has_contents() {
                    last_content_row = row;
                    break 'b;
                }
            }
        }
    }

    // Only render rows up to the last one with content
    let actual_rows = last_content_row + 1;

    let image_width = cols as u32 * char_width + padding * 2;
    let image_height = actual_rows as u32 * char_height + title_height + padding * 2;

    let mut canvas = Canvas::new(image_width, image_height)
        .map_err(|e| ScreenshotError::CanvasError(e.to_string()))?;

    canvas.set_char_size(char_width, char_height);

    // Fill background
    canvas.fill(config.background_color);

    // Draw title bar if decorations are enabled
    if config.show_decorations {
        let title = config.title.as_deref().unwrap_or("Terminal");
        canvas.draw_title_bar(title, config.padding);

        // Draw title with proper font
        let title_x = (padding + 8) as i32;
        let title_y = 10;
        canvas.draw_text_with_font(
            title,
            title_x,
            title_y,
            [220, 220, 220, 255],
            &font_data.font,
            font_data.scale,
        );
    }

    // Draw terminal content (only up to last_content_row)
    for row in 0..actual_rows {
        for col in 0..cols {
            if let Some(cell) = screen.cell(row, col) {
                let x = padding + col as u32 * char_width;
                let y = title_height + padding + row as u32 * char_height;

                // Draw background color
                let bg = cell.bgcolor();
                if bg != vt100::Color::Default {
                    let color = theme.color_to_rgba(bg);
                    let w = if cell.is_wide() {
                        char_width * 2
                    } else {
                        char_width
                    };
                    canvas.fill_rect(x as i32, y as i32, w, char_height, color);
                }

                // Draw text - imageproc's draw_text_mut y param is the top of the text
                if cell.has_contents() && !cell.is_wide_continuation() {
                    let fg = cell.fgcolor();
                    let fg_color = theme.get_foreground(fg, cell.bold(), cell.dim());
                    canvas.draw_text_with_font(
                        cell.contents(),
                        x as i32,
                        y as i32,
                        fg_color,
                        &font_data.font,
                        font_data.scale,
                    );
                }
            }
        }
    }

    canvas
        .to_image()
        .map_err(|e| ScreenshotError::CanvasError(e.to_string()))
}

/// Save a vt100::Screen to a PNG file
pub fn save_screen_png(
    screen: &vt100::Screen,
    path: &str,
    config: &ScreenshotConfig,
) -> Result<(), ScreenshotError> {
    let image = capture_screen(screen, config)?;
    image.save(path)?;
    Ok(())
}
