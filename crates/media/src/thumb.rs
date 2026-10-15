//! ThumbHash：客户端在图片加载前显示的模糊占位（约 25 字节，base64 后放进媒体对象）。
//!
//! 只解码 JPEG / PNG / GIF（第一帧）/ WebP；AVIF 没有纯 Rust 解码器，不算。
//! 解码的是不可信输入，限制尺寸与内存；任何失败都只是没有占位，不影响上传。

use std::io::Cursor;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use image::{ImageFormat, ImageReader, Limits};

/// ThumbHash 要求输入不超过 100×100。
const MAX_SIDE: u32 = 100;
/// 解码上限：单边 16384 像素、256 MiB 内存。
const MAX_DIMENSION: u32 = 16_384;
const MAX_ALLOC: u64 = 256 * 1024 * 1024;

/// 计算 ThumbHash（base64）。CPU 密集，调用方放到阻塞线程里跑。
pub fn compute(bytes: &[u8], mime: &str) -> Option<String> {
    let format = match mime {
        "image/jpeg" => ImageFormat::Jpeg,
        "image/png" => ImageFormat::Png,
        "image/gif" => ImageFormat::Gif,
        "image/webp" => ImageFormat::WebP,
        _ => return None,
    };
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_ALLOC);
    reader.limits(limits);
    let image = reader.decode().ok()?;
    let small = image.thumbnail(MAX_SIDE, MAX_SIDE).to_rgba8();
    let (w, h) = small.dimensions();
    let hash = thumbhash::rgba_to_thumb_hash(
        usize::try_from(w).ok()?,
        usize::try_from(h).ok()?,
        small.as_raw(),
    );
    Some(STANDARD.encode(hash))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_fn(w, h, |x, y| {
            image::Rgba([(x * 255 / w) as u8, (y * 255 / h) as u8, 128, 255])
        });
        let mut out = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn computes_for_real_images_and_skips_garbage() {
        let hash = compute(&png(320, 200), "image/png").unwrap();
        assert!(hash.len() > 10 && hash.len() < 60, "{hash}");
        assert_eq!(
            compute(&png(320, 200), "image/png").unwrap(),
            hash,
            "确定性"
        );
        assert!(compute(b"\x89PNG\r\n\x1a\nbroken", "image/png").is_none());
        assert!(compute(&png(8, 8), "image/avif").is_none());
    }
}
