//! 对外地址与缩略图地址（UPL-009、CDN-004/005/007）。

/// 后端统一的缩略图宽度（CDN-007）。
pub const THUMB_WIDTH: u32 = 360;
/// ESA 图片处理的原图大小上限（字节）。
const ESA_MAX_BYTES: u64 = 10_000_000;
/// ESA 图片处理的原图、结果宽×高上限。
const ESA_MAX_PIXELS: u64 = 16_777_216;
/// ESA 转 WebP 时单边上限。
const ESA_MAX_SIDE: u32 = 16_384;

/// 选缩略图服务要用到的图片信息（CDN-004）。
#[derive(Debug, Clone, Copy)]
pub struct ImageInfo<'a> {
    pub mime: &'a str,
    pub size_bytes: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

impl ImageInfo<'_> {
    /// 阿里云 ESA 能否处理这张图；不能时改走 R2（Cloudflare 图片变换）。
    ///
    /// ESA 的限制：原图不超过 10MB、宽×高不超过 16,777,216、转 WebP 时单边不超过 16,384；
    /// 格式不含 AVIF；GIF 的宽×高按所有帧相加，库里没有帧数，所以 GIF 一律走 R2。
    /// 宽高未知时按能处理算（沿用旧行为）。
    fn esa_can_process(&self) -> bool {
        if self.size_bytes > ESA_MAX_BYTES {
            return false;
        }
        if !matches!(self.mime, "image/jpeg" | "image/png" | "image/webp") {
            return false;
        }
        match (self.width, self.height) {
            (Some(w), Some(h)) => {
                u64::from(w) * u64::from(h) <= ESA_MAX_PIXELS
                    && w <= ESA_MAX_SIDE
                    && h <= ESA_MAX_SIDE
            }
            _ => true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Urls {
    /// 例如 `https://im.tiwat.cn`；为空时没有 CDN，缩略图就是原图。
    cdn_base: String,
    /// 没有 CDN 时的地址前缀 `{endpoint}/{bucket}`。
    origin_base: String,
}

impl Urls {
    pub fn new(public_base_url: &str, endpoint: &str, bucket: &str) -> Self {
        Self {
            cdn_base: public_base_url.trim_end_matches('/').to_owned(),
            origin_base: format!("{}/{}", endpoint.trim_end_matches('/'), bucket),
        }
    }

    pub fn public_url(&self, key: &str) -> String {
        if self.cdn_base.is_empty() {
            format!("{}/{key}", self.origin_base)
        } else {
            format!("{}/{key}", self.cdn_base)
        }
    }

    /// 360 宽 WebP 缩略图。没有 CDN 时返回原图地址。
    pub fn thumb_url(&self, key: &str, info: ImageInfo<'_>) -> String {
        if self.cdn_base.is_empty() {
            return self.public_url(key);
        }
        if !info.esa_can_process() {
            format!(
                "{}/cdn-cgi/image/width={THUMB_WIDTH},format=webp,quality=80/{key}",
                self.cdn_base
            )
        } else {
            format!(
                "{}/{key}?image_process=resize,w_{THUMB_WIDTH}/format,webp/quality,q_80",
                self.cdn_base
            )
        }
    }

    /// 原尺寸 WebP（论坛名片，COSMETIC-009、CDN-007）。没有 CDN 时返回原图地址。
    pub fn webp_url(&self, key: &str, info: ImageInfo<'_>) -> String {
        if self.cdn_base.is_empty() {
            return self.public_url(key);
        }
        if !info.esa_can_process() {
            format!(
                "{}/cdn-cgi/image/format=webp,quality=80/{key}",
                self.cdn_base
            )
        } else {
            format!(
                "{}/{key}?image_process=format,webp/quality,q_80",
                self.cdn_base
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jpg(size_bytes: u64, width: u32, height: u32) -> ImageInfo<'static> {
        ImageInfo {
            mime: "image/jpeg",
            size_bytes,
            width: Some(width),
            height: Some(height),
        }
    }

    const ESA_THUMB: &str =
        "https://im.tiwat.cn/a.jpg?image_process=resize,w_360/format,webp/quality,q_80";
    const R2_THUMB: &str =
        "https://im.tiwat.cn/cdn-cgi/image/width=360,format=webp,quality=80/a.jpg";

    #[test]
    fn urls() {
        let cdn = Urls::new("https://im.tiwat.cn/", "https://s3.example", "media");
        assert_eq!(
            cdn.public_url("2026/10/a.jpg"),
            "https://im.tiwat.cn/2026/10/a.jpg"
        );
        assert_eq!(cdn.thumb_url("a.jpg", jpg(1000, 800, 600)), ESA_THUMB);
        assert_eq!(cdn.thumb_url("a.jpg", jpg(20_000_000, 800, 600)), R2_THUMB);
        assert_eq!(
            cdn.webp_url("a.jpg", jpg(1000, 800, 600)),
            "https://im.tiwat.cn/a.jpg?image_process=format,webp/quality,q_80"
        );
        assert_eq!(
            cdn.webp_url("a.jpg", jpg(20_000_000, 800, 600)),
            "https://im.tiwat.cn/cdn-cgi/image/format=webp,quality=80/a.jpg"
        );
        let origin = Urls::new("", "http://127.0.0.1:9000/", "media");
        assert_eq!(
            origin.webp_url("a.jpg", jpg(1, 1, 1)),
            "http://127.0.0.1:9000/media/a.jpg"
        );
        assert_eq!(
            origin.public_url("a.jpg"),
            "http://127.0.0.1:9000/media/a.jpg"
        );
        assert_eq!(
            origin.thumb_url("a.jpg", jpg(1, 1, 1)),
            "http://127.0.0.1:9000/media/a.jpg"
        );
    }

    #[test]
    fn esa_limits() {
        let cdn = Urls::new("https://im.tiwat.cn", "https://s3.example", "media");
        // 宽×高恰好 16,777,216 还能处理，多一行就不行。
        assert_eq!(cdn.thumb_url("a.jpg", jpg(1000, 4096, 4096)), ESA_THUMB);
        assert_eq!(cdn.thumb_url("a.jpg", jpg(1000, 4096, 4097)), R2_THUMB);
        // 小于 10MB 但像素超限（实际帖子：10000×4218）。
        assert_eq!(
            cdn.thumb_url("a.jpg", jpg(5_000_000, 10_000, 4218)),
            R2_THUMB
        );
        // 单边超过 16,384。
        assert_eq!(cdn.thumb_url("a.jpg", jpg(1000, 16_385, 1000)), R2_THUMB);
        // 宽高未知按能处理算。
        let unknown = ImageInfo {
            mime: "image/png",
            size_bytes: 1000,
            width: None,
            height: None,
        };
        assert_eq!(cdn.thumb_url("a.jpg", unknown), ESA_THUMB);
        // GIF（帧数未知）与 AVIF 走 R2。
        for mime in ["image/gif", "image/avif"] {
            let info = ImageInfo {
                mime,
                size_bytes: 1000,
                width: Some(10),
                height: Some(10),
            };
            assert_eq!(cdn.thumb_url("a.jpg", info), R2_THUMB, "{mime}");
        }
    }
}
