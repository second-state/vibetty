use vibetty::screenshot::font::FontData;

fn main() {
    match FontData::new(14.0) {
        Ok(font) => {
            println!("Font loaded successfully!");
            println!("Font scale: {:?}", font.scale);
            let space_id = font.font.glyph_id(' ');
            let advance = font.font.h_advance_unscaled(space_id);
            println!("Space advance: {}", advance);
        }
        Err(e) => {
            println!("Failed to load font: {}", e);
        }
    }
}
