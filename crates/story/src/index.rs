//! 字符二元组倒排索引。
//!
//! 中文没有天然分词，按「相邻两个字」建倒排是子串检索最快也最准的做法：
//! 查询词的每个二元组都必须出现在候选行里，取交集后（三个字以上）再逐行核对一次子串。
//! 每行末尾补一个 `(末字, 0)` 键，于是任何单字都是某个键的首字，
//! 单字查询就变成按首字取一段连续的键再求并集。
//!
//! 存储为 CSR：`keys` 有序，`offsets[i]..offsets[i+1]` 是 `postings` 中第 i 个键的行号区间。

const END: u32 = 0;

#[derive(Debug, Default)]
pub struct BigramIndex {
    line_count: u32,
    keys: Vec<u64>,
    offsets: Vec<u32>,
    postings: Vec<u32>,
}

#[inline]
fn key(a: char, b: u32) -> u64 {
    (u64::from(a as u32) << 32) | u64::from(b)
}

/// 一次查询的候选行（升序）。
pub enum Candidates {
    /// 候选即命中，无需核对。
    Exact(Vec<u32>),
    /// 只保证包含全部二元组，需要逐行核对子串。
    Verify(Vec<u32>),
}

impl BigramIndex {
    /// `folded_lines` 是已经折叠过的每行文本。
    pub fn build(folded_lines: &[String]) -> Self {
        let total: usize = folded_lines.iter().map(|l| l.len() / 2 + 1).sum();
        let mut pairs: Vec<(u64, u32)> = Vec::with_capacity(total);
        let mut keys: Vec<u64> = Vec::new();
        for (line, text) in folded_lines.iter().enumerate() {
            keys.clear();
            let mut chars = text.chars().peekable();
            while let Some(a) = chars.next() {
                let b = chars.peek().map_or(END, |&c| c as u32);
                keys.push(key(a, b));
            }
            keys.sort_unstable();
            keys.dedup();
            let line = line as u32;
            pairs.extend(keys.iter().map(|&k| (k, line)));
        }
        pairs.sort_unstable();

        let mut keys = Vec::new();
        let mut offsets = Vec::new();
        let mut postings = Vec::with_capacity(pairs.len());
        for (k, line) in pairs {
            if keys.last() != Some(&k) {
                keys.push(k);
                offsets.push(postings.len() as u32);
            }
            postings.push(line);
        }
        offsets.push(postings.len() as u32);
        Self {
            line_count: folded_lines.len() as u32,
            keys,
            offsets,
            postings,
        }
    }

    pub fn key_count(&self) -> usize {
        self.keys.len()
    }

    pub fn posting_count(&self) -> usize {
        self.postings.len()
    }

    fn postings_for(&self, k: u64) -> &[u32] {
        match self.keys.binary_search(&k) {
            Ok(i) => &self.postings[self.offsets[i] as usize..self.offsets[i + 1] as usize],
            Err(_) => &[],
        }
    }

    /// 已折叠检索词 → 候选行。
    pub fn candidates(&self, term: &str) -> Candidates {
        let chars: Vec<char> = term.chars().collect();
        match chars.len() {
            0 => Candidates::Exact(Vec::new()),
            1 => Candidates::Exact(self.single_char(chars[0])),
            2 => Candidates::Exact(self.postings_for(key(chars[0], chars[1] as u32)).to_vec()),
            _ => {
                let mut keys: Vec<u64> = chars.windows(2).map(|w| key(w[0], w[1] as u32)).collect();
                keys.sort_unstable();
                keys.dedup();
                let mut lists: Vec<&[u32]> = keys.iter().map(|&k| self.postings_for(k)).collect();
                lists.sort_by_key(|l| l.len());
                Candidates::Verify(intersect_all(&lists))
            }
        }
    }

    fn single_char(&self, c: char) -> Vec<u32> {
        let lo = key(c, 0);
        let hi = lo | u64::from(u32::MAX);
        let from = self.keys.partition_point(|&k| k < lo);
        let to = self.keys.partition_point(|&k| k <= hi);
        if from == to {
            return Vec::new();
        }
        let slice = &self.postings[self.offsets[from] as usize..self.offsets[to] as usize];
        if to - from == 1 {
            return slice.to_vec();
        }
        // 结果大时用位图去重，小时直接排序。
        if slice.len() > self.line_count as usize / 32 {
            let mut bits = vec![0u64; (self.line_count as usize).div_ceil(64)];
            for &line in slice {
                bits[line as usize >> 6] |= 1 << (line & 63);
            }
            let mut out = Vec::new();
            for (word_idx, &word) in bits.iter().enumerate() {
                let mut w = word;
                while w != 0 {
                    let bit = w.trailing_zeros();
                    out.push((word_idx as u32) << 6 | bit);
                    w &= w - 1;
                }
            }
            out
        } else {
            let mut out = slice.to_vec();
            out.sort_unstable();
            out.dedup();
            out
        }
    }
}

/// 多个有序列表求交，`lists` 已按长度升序。
fn intersect_all(lists: &[&[u32]]) -> Vec<u32> {
    let Some((first, rest)) = lists.split_first() else {
        return Vec::new();
    };
    let mut acc = first.to_vec();
    for other in rest {
        if acc.is_empty() {
            break;
        }
        acc = intersect_small_large(&acc, other);
    }
    acc
}

/// 两个有序列表求交（任意长度顺序）。
pub fn intersect(a: &[u32], b: &[u32]) -> Vec<u32> {
    if a.len() <= b.len() {
        intersect_small_large(a, b)
    } else {
        intersect_small_large(b, a)
    }
}

fn intersect_small_large(small: &[u32], large: &[u32]) -> Vec<u32> {
    let mut out = Vec::with_capacity(small.len());
    if small.len() * 16 < large.len() {
        // 长短悬殊：在长表上指数搜索。
        let mut base = 0;
        for &x in small {
            let rest = &large[base..];
            let mut bound = 1;
            while bound < rest.len() && rest[bound] < x {
                bound <<= 1;
            }
            let window = &rest[bound / 2..rest.len().min(bound + 1)];
            match window.binary_search(&x) {
                Ok(i) => {
                    out.push(x);
                    base += bound / 2 + i + 1;
                }
                Err(i) => base += bound / 2 + i,
            }
            if base >= large.len() {
                break;
            }
        }
    } else {
        let (mut i, mut j) = (0, 0);
        while i < small.len() && j < large.len() {
            match small[i].cmp(&large[j]) {
                std::cmp::Ordering::Less => i += 1,
                std::cmp::Ordering::Greater => j += 1,
                std::cmp::Ordering::Equal => {
                    out.push(small[i]);
                    i += 1;
                    j += 1;
                }
            }
        }
    }
    out
}

/// 两个升序去重列表求并。
pub fn union_sorted(a: &[u32], b: &[u32]) -> Vec<u32> {
    let mut out = Vec::with_capacity(a.len() + b.len());
    let (mut i, mut j) = (0, 0);
    while i < a.len() || j < b.len() {
        let x = match (a.get(i), b.get(j)) {
            (Some(&x), Some(&y)) if x == y => {
                i += 1;
                j += 1;
                x
            }
            (Some(&x), Some(&y)) if x < y => {
                i += 1;
                x
            }
            (Some(_), Some(&y)) | (None, Some(&y)) => {
                j += 1;
                y
            }
            (Some(&x), None) => {
                i += 1;
                x
            }
            (None, None) => break,
        };
        out.push(x);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(src: &[&str]) -> Vec<String> {
        src.iter().map(|s| crate::text::fold(s)).collect()
    }

    fn run(index: &BigramIndex, folded: &[String], term: &str) -> Vec<u32> {
        match index.candidates(term) {
            Candidates::Exact(v) => v,
            Candidates::Verify(v) => v
                .into_iter()
                .filter(|&l| folded[l as usize].contains(term))
                .collect(),
        }
    }

    #[test]
    fn finds_substrings_of_any_length() {
        let folded = lines(&[
            "狡兔屋的妮可",
            "妮可·德玛拉",
            "铃",
            "哲和铃在录像店",
            "Fairy",
            "屋兔狡",
        ]);
        let index = BigramIndex::build(&folded);
        assert_eq!(run(&index, &folded, "妮可"), vec![0, 1]);
        assert_eq!(run(&index, &folded, "铃"), vec![2, 3]);
        assert_eq!(run(&index, &folded, "狡兔屋"), vec![0]);
        assert_eq!(run(&index, &folded, "录像店"), vec![3]);
        assert_eq!(run(&index, &folded, "fairy"), vec![4]);
        assert_eq!(run(&index, &folded, "可"), vec![0, 1]);
        assert!(run(&index, &folded, "不存在").is_empty());
        assert!(run(&index, &folded, "").is_empty());
    }

    #[test]
    fn single_char_bitmap_path_matches_sort_path() {
        let src: Vec<String> = (0..500)
            .map(|i| {
                if i % 3 == 0 {
                    format!("铃{i}")
                } else {
                    format!("哲{i}")
                }
            })
            .collect();
        let index = BigramIndex::build(&src);
        let got = run(&index, &src, "铃");
        let want: Vec<u32> = (0..500).filter(|i| i % 3 == 0).collect();
        assert_eq!(got, want);
    }

    #[test]
    fn intersect_both_strategies() {
        let large: Vec<u32> = (0..10_000).collect();
        assert_eq!(
            intersect(&[3, 500, 9_999, 20_000], &large),
            vec![3, 500, 9_999]
        );
        assert_eq!(intersect(&[1, 2, 3], &[2, 3, 4]), vec![2, 3]);
        assert!(intersect(&[], &large).is_empty());
        assert_eq!(intersect(&large[..5], &[1, 3]), vec![1, 3]);
    }

    #[test]
    fn union_merges() {
        assert_eq!(union_sorted(&[1, 3, 5], &[2, 3, 6]), vec![1, 2, 3, 5, 6]);
        assert_eq!(union_sorted(&[], &[4]), vec![4]);
        assert_eq!(union_sorted(&[4], &[]), vec![4]);
    }
}
