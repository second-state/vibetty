//! Utility functions for screenshot rendering

/// Calculate character size based on font size
#[allow(dead_code)]
pub fn calculate_char_size(font_size: f32) -> (u32, u32) {
    let width = (font_size * 0.6) as u32;
    let height = (font_size * 1.2) as u32;
    (width, height)
}

/// Clamp a value between min and max
#[allow(dead_code)]
pub fn clamp<T: Ord>(value: T, min: T, max: T) -> T {
    if value < min {
        min
    } else if value > max {
        max
    } else {
        value
    }
}
