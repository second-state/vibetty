//! 端到端验证 vibetty 的 `encode_paletted_png`。
//!
//! 用 `#[path]` 把 vibetty 自己的 `src/png_encode.rs` include 进来,
//! 拿一张真实终端截图跑一遍,对照 image 默认编码的大小,
//! 再用 image 解回来确认 round-trip,并给出 PSNR。
//!
//! 用法: cargo run --example verify_png [path/to/screenshot.png]

#[path = "../src/png_encode.rs"]
mod png_encode;

use image::{ExtendedColorType, ImageEncoder, ImageReader};

fn main() {
    let input = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/Users/chensiheng/Downloads/screenshot.png".to_string());

    let img = ImageReader::open(&input)
        .unwrap()
        .with_guessed_format()
        .unwrap()
        .decode()
        .expect("读不开输入图");

    // (A) 对照:image 默认 PngEncoder —— 即 ws.rs 改之前用的编码
    let mut default_png = Vec::new();
    let enc = image::codecs::png::PngEncoder::new(&mut default_png);
    enc.write_image(
        img.as_bytes(),
        img.width(),
        img.height(),
        ExtendedColorType::from(img.color()),
    )
    .unwrap();

    // (B) vibetty 新的 palette 编码(直接调本 crate 的函数)
    let paletted = png_encode::encode_paletted_png(&img).expect("encode_paletted_png 失败");

    let k = |n: usize| n as f64 / 1024.0;
    println!(
        "输入                   : {input}  ({}x{})",
        img.width(),
        img.height()
    );
    println!("image 默认 PNG (对照)  : {:>6.1}K", k(default_png.len()));
    println!(
        "vibetty palette PNG    : {:>6.1}K  ({:.2}x 压缩)",
        k(paletted.len()),
        default_png.len() as f64 / paletted.len() as f64
    );

    // 落盘 + 用 image 解回来
    let out = "/tmp/vibetty_pal.png";
    std::fs::write(out, &paletted).unwrap();
    let back = ImageReader::open(out)
        .unwrap()
        .decode()
        .expect("image 解不开这张 palette PNG");

    // PSNR vs 原图
    let orig = img.to_rgb8();
    let back_rgb = back.to_rgb8();
    assert_eq!(orig.dimensions(), back_rgb.dimensions());
    let mut sum_sq: u64 = 0;
    for (a, b) in orig.pixels().zip(back_rgb.pixels()) {
        for c in 0..3 {
            let d = a[c] as i32 - b[c] as i32;
            sum_sq += (d * d) as u64;
        }
    }
    let total = (orig.width() as usize) * (orig.height() as usize) * 3;
    let mse = sum_sq as f64 / total as f64;
    let psnr = if mse > 0.0 {
        10.0 * (255.0 * 255.0 / mse).log10()
    } else {
        f64::INFINITY
    };

    println!(
        "image 解回成功          : {}x{}  color={:?}",
        back.width(),
        back.height(),
        back.color()
    );
    println!("PSNR vs 原图            : {:.2} dB  (>40≈视觉无损)", psnr);
    println!("产物                   : {out}  (打开核一眼质量)");
}
