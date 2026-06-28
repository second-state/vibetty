//! Palette (indexed) PNG encoding for terminal screenshots.
//!
//! Replaces image-crate's default RGBA PNG encoder. NeuQuant 256-color
//! quantization + `png::Compression::Best` gives ~3-4x smaller output at
//! near-lossless quality (PSNR ~49dB) on typical terminal content.

use anyhow::{Context, Result};
use color_quant::NeuQuant;
use image::DynamicImage;
use png::{BitDepth, ColorType, Compression, Encoder};

/// Encode a screenshot as a near-lossless palette (indexed) PNG.
///
/// Input is expected to be opaque RGBA (terminal frames are alpha=255).
/// Output is PNG color-type 3 with a 256-entry palette.
pub fn encode_paletted_png(image: &DynamicImage) -> Result<Vec<u8>> {
    let rgba = image.to_rgba8();
    let (w, h) = rgba.dimensions();
    // NeuQuant 训练和查询都要求 RGBA(4字节/像素),不是 RGB
    let rgba_raw = rgba.as_raw();

    let nq = NeuQuant::new(10, 256, rgba_raw);
    let palette = nq.color_map_rgb(); // 256 × 3 字节调色板
    let indices: Vec<u8> = rgba_raw
        .chunks_exact(4)
        .map(|p| nq.index_of(p) as u8)
        .collect();

    let mut out = Vec::with_capacity(indices.len());
    {
        let mut enc = Encoder::new(&mut out, w, h);
        enc.set_color(ColorType::Indexed);
        enc.set_depth(BitDepth::Eight);
        enc.set_palette(&palette);
        enc.set_compression(Compression::Best); // 关键:image 默认压缩弱很多
        let mut writer = enc.write_header().context("write PNG header")?;
        writer
            .write_image_data(&indices)
            .context("write PNG image data")?;
    }
    Ok(out)
}
