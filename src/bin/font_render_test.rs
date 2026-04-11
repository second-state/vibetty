//! Test program for font rendering

use image::RgbaImage;
use vt100::Parser;
use ab_glyph::Font;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Testing font rendering...");

    // Load font
    let font_data = screenshot::font::FontData::new(14.0)
        .unwrap_or_else(|_| screenshot::font::FontData::with_fallback(14.0));

    println!("✓ Font loaded successfully!");
    let space_id = font_data.font.glyph_id(' ');
    let advance = font_data.font.h_advance_unscaled(space_id);
    println!("  Space advance (unscaled): {}", advance);
    println!("  Font scale: {:?}", font_data.scale);

    // Create a simple terminal screen with colored text
    let mut parser = Parser::new(10, 60, 8096);

    parser.process(b"\x1b[31mRed Text\x1b[0m\r\n");
    parser.process(b"\x1b[32mGreen Text\x1b[0m\r\n");
    parser.process(b"\x1b[33mYellow Text\x1b[0m\r\n");
    parser.process(b"\x1b[34mBlue Text\x1b[0m\r\n");
    parser.process(b"\x1b[1mBold Text\x1b[0m\r\n");
    parser.process(b"Normal text with some content\r\n");
    parser.process(b"The quick brown fox jumps over the lazy dog\r\n");

    let screen = parser.screen();
    let (rows, cols) = screen.size();

    // Calculate character dimensions
    // Get the space glyph and calculate its advance width in pixels
    let space_id = font_data.font.glyph_id(' ');
    let units_per_em = font_data.font.units_per_em().unwrap_or(2048.0);
    let advance_unscaled = font_data.font.h_advance_unscaled(space_id);
    // Convert to pixels: (advance / units_per_em) * scale_x
    let char_width = ((advance_unscaled / units_per_em) * font_data.scale.x).round() as u32;

    // Character height = ascent + descent (proper line height)
    let ascent = (font_data.font.ascent_unscaled() / units_per_em * font_data.scale.y).round() as u32;
    let descent = (font_data.font.descent_unscaled() / units_per_em * font_data.scale.y).round() as u32;
    let char_height = ascent + descent;

    println!("\nCharacter size: {}x{}", char_width, char_height);

    let padding = 16u32;
    let title_height = 32u32;
    let image_width = cols as u32 * char_width + padding * 2;
    let image_height = rows as u32 * char_height + title_height + padding * 2;

    println!("Image size: {}x{}", image_width, image_height);

    let mut canvas = screenshot::canvas::Canvas::new(image_width, image_height)
        .map_err(|e| format!("Failed to create canvas: {}", e))?;

    canvas.fill([30, 30, 30, 255]);

    // Draw title bar
    canvas.fill_rect(0, 0, image_width, title_height, [40, 40, 45, 255]);
    canvas.fill_rect(0, 30, image_width, 2, [60, 60, 65, 255]);
    canvas.draw_text_simple("Font Render Test", padding as i32 + 8, 10, [220, 220, 220, 255]);

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

                // Draw text - imageproc's draw_text_mut y param is the top of the text
                if cell.has_contents() && !cell.is_wide_continuation() {
                    let fg = cell.fgcolor();
                    let fg_color = color_to_rgba_with_bold(fg, cell.bold());
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

    // Save image
    let image = canvas.to_image()
        .map_err(|e| format!("Failed to create image: {}", e))?;
    image.save("font_render_test.png")?;
    println!("\n✓ Screenshot saved to font_render_test.png");
    println!("  Size: {}x{}", image.width(), image.height());

    Ok(())
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

fn color_to_rgba_with_bold(color: vt100::Color, bold: bool) -> [u8; 4] {
    let mut rgba = color_to_rgba(color);
    if bold {
        rgba[0] = ((rgba[0] as u16 + 255) / 2) as u8;
        rgba[1] = ((rgba[1] as u16 + 255) / 2) as u8;
        rgba[2] = ((rgba[2] as u16 + 255) / 2) as u8;
    }
    rgba
}

// Include screenshot modules
mod screenshot {
    pub mod canvas;
    pub mod theme;
    pub mod font;
}
