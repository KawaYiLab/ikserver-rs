//! 对外地址与缩略图地址（UPL-009、CDN-004/005/007）。

/// 后端统一的缩略图宽度（CDN-007）。
pub const THUMB_WIDTH: u32 = 360;
/// 大于这个字节数的图走 R2 的图片变换，否则走阿里云 ESA（CDN-004）。
const ESA_SIZE_THRESHOLD: u64 = 10_000_000;

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
    pub fn thumb_url(&self, key: &str, size_bytes: u64) -> String {
        if self.cdn_base.is_empty() {
            return self.public_url(key);
        }
        if size_bytes > ESA_SIZE_THRESHOLD {
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
    pub fn webp_url(&self, key: &str, size_bytes: u64) -> String {
        if self.cdn_base.is_empty() {
            return self.public_url(key);
        }
        if size_bytes > ESA_SIZE_THRESHOLD {
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

    #[test]
    fn urls() {
        let cdn = Urls::new("https://im.tiwat.cn/", "https://s3.example", "media");
        assert_eq!(
            cdn.public_url("2026/10/a.jpg"),
            "https://im.tiwat.cn/2026/10/a.jpg"
        );
        assert_eq!(
            cdn.thumb_url("a.jpg", 1000),
            "https://im.tiwat.cn/a.jpg?image_process=resize,w_360/format,webp/quality,q_80"
        );
        assert_eq!(
            cdn.thumb_url("a.jpg", 20_000_000),
            "https://im.tiwat.cn/cdn-cgi/image/width=360,format=webp,quality=80/a.jpg"
        );
        assert_eq!(
            cdn.webp_url("a.jpg", 1000),
            "https://im.tiwat.cn/a.jpg?image_process=format,webp/quality,q_80"
        );
        assert_eq!(
            cdn.webp_url("a.jpg", 20_000_000),
            "https://im.tiwat.cn/cdn-cgi/image/format=webp,quality=80/a.jpg"
        );
        let origin = Urls::new("", "http://127.0.0.1:9000/", "media");
        assert_eq!(
            origin.webp_url("a.jpg", 1),
            "http://127.0.0.1:9000/media/a.jpg"
        );
        assert_eq!(
            origin.public_url("a.jpg"),
            "http://127.0.0.1:9000/media/a.jpg"
        );
        assert_eq!(
            origin.thumb_url("a.jpg", 1),
            "http://127.0.0.1:9000/media/a.jpg"
        );
    }
}
