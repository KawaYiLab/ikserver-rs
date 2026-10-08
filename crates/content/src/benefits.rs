//! 等级权益矩阵（POST-012）。下标是 `Principal::benefit_level()`（0–7）。
//! 图片上限（`[0,9,12,15,18,24,30,40]`）随媒体模块一起接入。

/// 正文最多字符数（按 Unicode 字符计）。等级 0 = 未通过考试，本来就不能发帖。
const MAX_BODY_CHARS: [usize; 8] = [0, 3000, 4000, 6000, 8000, 10000, 13000, 16000];

pub fn max_body_chars(level: usize) -> usize {
    MAX_BODY_CHARS[level.min(7)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_are_clamped() {
        assert_eq!(max_body_chars(0), 0);
        assert_eq!(max_body_chars(1), 3000);
        assert_eq!(max_body_chars(7), 16000);
        assert_eq!(max_body_chars(99), 16000);
    }
}
