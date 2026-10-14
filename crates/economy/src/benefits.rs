//! 等级权益矩阵（BENEFIT-001）。下标是权益等级（0–7，0 = 未通过考试）。
//! 服务端是唯一裁决方（BENEFIT-003），前端只拿来展示。

use crate::dto::Benefits;

const ARTICLE_MAX_IMAGES: [u16; 8] = [0, 9, 12, 15, 18, 24, 30, 40];
const COMMENT_MAX_IMAGES: [u16; 8] = [0, 3, 6, 9, 9, 12, 12, 18];
const POST_MAX_BODY_CHARS: [u32; 8] = [0, 3000, 4000, 6000, 8000, 10000, 13000, 16000];

pub const MAX_BENEFIT_LEVEL: usize = 7;

pub fn for_level(level: usize) -> Benefits {
    let i = level.min(MAX_BENEFIT_LEVEL);
    Benefits {
        post_max_images: ARTICLE_MAX_IMAGES[i],
        comment_max_images: COMMENT_MAX_IMAGES[i],
        post_max_body_chars: POST_MAX_BODY_CHARS[i],
    }
}

/// 帖子正文最多字符数（按 Unicode 字符计）。
pub fn max_post_body_chars(level: usize) -> usize {
    usize::try_from(for_level(level).post_max_body_chars).unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_are_clamped() {
        assert_eq!(max_post_body_chars(0), 0);
        assert_eq!(max_post_body_chars(1), 3000);
        assert_eq!(max_post_body_chars(7), 16000);
        assert_eq!(max_post_body_chars(99), 16000);
        assert_eq!(for_level(4).comment_max_images, 9);
        assert_eq!(for_level(7).post_max_images, 40);
    }
}
