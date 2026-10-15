//! 允许的图片类型、扩展名与魔数嗅探（UPL-001/002/005、UPL-033）。真实类型只信嗅探结果。

/// 单文件上限 30 MiB（UPL-003）。
pub const MAX_UPLOAD_BYTES: u64 = 30 * 1024 * 1024;

/// (MIME, 允许的扩展名；第一个是规范扩展名)。
const TYPES: &[(&str, &[&str])] = &[
    ("image/jpeg", &["jpg", "jpeg"]),
    ("image/png", &["png"]),
    ("image/gif", &["gif"]),
    ("image/webp", &["webp"]),
    ("image/avif", &["avif"]),
];

/// 规范化客户端声明的 MIME：去空白、小写、`image/jpg` → `image/jpeg`；不在白名单时为 None。
pub fn normalize_mime(mime: &str) -> Option<&'static str> {
    let mime = mime.trim().to_ascii_lowercase();
    let mime = if mime == "image/jpg" {
        "image/jpeg"
    } else {
        mime.as_str()
    };
    TYPES.iter().find(|(m, _)| *m == mime).map(|(m, _)| *m)
}

/// 规范扩展名（带点），例如 `.jpg`。
pub fn canonical_ext(mime: &str) -> &'static str {
    match mime {
        "image/jpeg" => ".jpg",
        "image/png" => ".png",
        "image/gif" => ".gif",
        "image/webp" => ".webp",
        _ => ".avif",
    }
}

/// 文件名的扩展名是否属于允许的图片扩展名全集（不要求与 MIME 一一对应，UPL-005）。
pub fn extension_allowed(filename: &str) -> bool {
    let Some((stem, ext)) = filename.rsplit_once('.') else {
        return false;
    };
    let ext = ext.to_ascii_lowercase();
    !stem.is_empty() && TYPES.iter().any(|(_, exts)| exts.contains(&ext.as_str()))
}

/// 按文件头识别图片类型。不认识或不在白名单时为 None。
pub fn sniff(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    // ISO BMFF：`????ftyp` + 主品牌或兼容品牌里有 avif / avis。
    if bytes.len() >= 16 && &bytes[4..8] == b"ftyp" {
        let size = usize::try_from(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
            .unwrap_or(0)
            .clamp(16, bytes.len().min(256));
        let major = &bytes[8..12];
        let (compatible, _) = bytes[16..size].as_chunks::<4>();
        let is_avif = |brand: &[u8]| brand == b"avif" || brand == b"avis";
        if is_avif(major) || compatible.iter().any(|b| is_avif(b)) {
            return Some("image/avif");
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_normalization() {
        assert_eq!(normalize_mime(" Image/JPG "), Some("image/jpeg"));
        assert_eq!(normalize_mime("image/webp"), Some("image/webp"));
        assert_eq!(normalize_mime("image/svg+xml"), None);
        assert_eq!(canonical_ext("image/jpeg"), ".jpg");
    }

    #[test]
    fn extensions() {
        assert!(extension_allowed("a.JPEG"));
        assert!(extension_allowed("photo.png.webp"));
        assert!(!extension_allowed("noext"));
        assert!(!extension_allowed("trailing."));
        assert!(!extension_allowed(".png"));
        assert!(!extension_allowed("x.svg"));
    }

    #[test]
    fn sniffing() {
        assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0]), Some("image/jpeg"));
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n...."), Some("image/png"));
        assert_eq!(sniff(b"GIF89a....."), Some("image/gif"));
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        let avif = b"\0\0\0\x1cftypmif1\0\0\0\0mif1avifmiaf";
        assert_eq!(sniff(avif), Some("image/avif"));
        assert_eq!(
            sniff(b"\0\0\0\x18ftypmp42\0\0\0\0mp42isom"),
            None,
            "MP4 不是图片"
        );
        assert_eq!(sniff(b"<svg xmlns=..."), None);
        assert_eq!(sniff(b""), None);
    }
}
