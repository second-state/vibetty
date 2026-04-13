//! Test program for font rendering

// Reuse the main screenshot module
#[path = "../screenshot/mod.rs"]
mod screenshot;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Testing font rendering...");

    // Create a simple terminal screen with colored text
    let mut parser = vt100::Parser::new(10, 60, 8096);

    parser.process(b"\x1b[31mRed Text\x1b[0m\r\n");
    parser.process(b"\x1b[32mGreen Text\x1b[0m\r\n");
    parser.process(b"\x1b[33mYellow Text\x1b[0m\r\n");
    parser.process(b"\x1b[34mBlue Text\x1b[0m\r\n");
    parser.process(b"\x1b[1mBold Text\x1b[0m\r\n");
    parser.process(b"Normal text with some content\r\n");
    parser.process(b"The quick brown fox jumps over the lazy dog\r\n");

    // Save screenshot using the library function
    let config = screenshot::ScreenshotConfig {
        title: Some("Font Render Test".to_string()),
        ..Default::default()
    };

    screenshot::save_screen_png(&parser.screen(), "font_render_test.png", &config)?;

    println!("✓ Screenshot saved to font_render_test.png");

    Ok(())
}
