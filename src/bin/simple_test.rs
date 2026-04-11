use ab_glyph::{Font, FontArc, PxScale};
use image::{ImageBuffer, Rgba, RgbaImage};
use tiny_skia::{Color, Paint, Pixmap, Rect, Transform};

fn main() {
    let font_data = std::fs::read("/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf").unwrap();
    let font = FontArc::try_from_slice(&font_data).unwrap();
    let scale = PxScale::from(14.0);
    
    let width = 400u32;
    let height = 100u32;
    
    let mut canvas = Pixmap::new(width, height).unwrap();
    canvas.fill(Color::from_rgba8(30, 30, 30, 255));
    
    // Draw baseline reference line at y=50
    canvas.fill_rect(0, 50, width, 1, [100, 100, 100, 255]);
    
    let rgba = image::Rgba([220, 220, 220, 255]);
    let mut text_layer = ImageBuffer::new(width, height);
    
    // Draw text on baseline (y=50)
    imageproc::drawing::draw_text_mut(&mut text_layer, rgba, 10, 50, scale, &font, "ABCDEfghij");
    
    // Draw text below baseline (y=70)
    imageproc::drawing::draw_text_mut(&mut text_layer, rgba, 10, 70, scale, &font, "ABCDEfghij");
    
    // Combine layers
    let mut final_image = RgbaImage::from_raw(width, height, canvas.data().to_vec()).unwrap();
    for (final_pixel, text_pixel) in final_image.pixels_mut().zip(text_layer.pixels()) {
        let alpha = text_pixel[3] as f32 / 255.0;
        if alpha > 0.0 {
            for i in 0..3 {
                final_pixel[i] = (text_pixel[i] as f32 * alpha + final_pixel[i] as f32 * (1.0 - alpha)) as u8;
            }
            final_pixel[3] = 255;
        }
    }
    
    final_image.save("alignment_test.png").unwrap();
    println!("Saved alignment_test.png");
}
