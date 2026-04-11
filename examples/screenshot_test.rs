//! Test script for screenshot functionality

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 直接使用 screenshot 模块
    use screenshot::{save_screen_png, ScreenshotConfig};
    use vt100::Parser;

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

    // Save screenshot
    let config = ScreenshotConfig {
        title: Some("Test Terminal".to_string()),
        show_decorations: true,
        ..Default::default()
    };

    save_screen_png(screen, "test_screenshot.png", &config)?;
    println!("Screenshot saved to test_screenshot.png");

    Ok(())
}
