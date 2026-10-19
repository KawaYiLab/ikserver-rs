//! 语料包：一次提取的全部产出打成一个 JSON 对象（上传时 gzip 压缩）。
//!
//! ```json
//! {"format": 1, "manifest": {…}, "story": "<story.jsonl 原文>",
//!  "supplement": "<supplement.jsonl 原文>", "lore": {"agents/01_狡兔屋.md": "…"}}
//! ```
//!
//! zzz-story-extractor 的 `python -m zzz_story upload` 生成它并上传到 `PUT /v1/admin/story/corpus`；
//! 本地开发时也可以直接从输出目录读（[`Bundle::from_dir`]）。

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// 当前包格式。结构有不兼容变化时递增，旧客户端上传会得到明确的错误。
pub const BUNDLE_FORMAT: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bundle {
    pub format: u32,
    /// manifest.json 的内容（数据版本、语言、生成时间等），可缺省。
    #[serde(default)]
    pub manifest: Option<serde_json::Value>,
    /// story.jsonl 原文。
    pub story: String,
    /// supplement.jsonl 原文，可缺省。
    #[serde(default)]
    pub supplement: Option<String>,
    /// lore 目录下的 Markdown：相对路径（`/` 分隔）→ 内容。
    #[serde(default)]
    pub lore: BTreeMap<String, String>,
}

impl Bundle {
    /// 从 zzz-story-extractor 的输出目录读取。
    pub fn from_dir(dir: &Path) -> Result<Self> {
        let story_path = dir.join("story.jsonl");
        if !story_path.is_file() {
            bail!(
                "{} 不是 zzz-story-extractor 的输出目录（缺少 story.jsonl）。先运行 `python -m zzz_story extract`",
                dir.display()
            );
        }
        let read = |p: &Path| {
            std::fs::read_to_string(p).with_context(|| format!("读取 {} 失败", p.display()))
        };
        let manifest = match std::fs::read(dir.join("manifest.json")) {
            Ok(bytes) => Some(serde_json::from_slice(&bytes).context("manifest.json 解析失败")?),
            Err(_) => None,
        };
        let supplement_path = dir.join("supplement.jsonl");
        let supplement = if supplement_path.is_file() {
            Some(read(&supplement_path)?)
        } else {
            None
        };
        let mut lore = BTreeMap::new();
        let lore_dir = dir.join("lore");
        if lore_dir.is_dir() {
            collect_md(&lore_dir, &lore_dir, &mut lore)?;
        }
        Ok(Self {
            format: BUNDLE_FORMAT,
            manifest,
            story: read(&story_path)?,
            supplement,
            lore,
        })
    }

    /// 解码上传的包：gzip（按魔数识别）或未压缩的 JSON。解压后超过 `max_bytes` 视为错误（防压缩炸弹）。
    pub fn decode(bytes: &[u8], max_bytes: usize) -> Result<Self> {
        let json: Vec<u8> = if bytes.starts_with(&[0x1f, 0x8b]) {
            let mut out = Vec::new();
            flate2::read::GzDecoder::new(bytes)
                .take(max_bytes as u64 + 1)
                .read_to_end(&mut out)
                .context("gzip 解压失败")?;
            if out.len() > max_bytes {
                bail!("解压后超过 {} MB 上限", max_bytes / 1_000_000);
            }
            out
        } else {
            bytes.to_vec()
        };
        let bundle: Self = serde_json::from_slice(&json).context("语料包不是合法的 JSON")?;
        if bundle.format != BUNDLE_FORMAT {
            bail!(
                "语料包格式 {} 不受支持（服务端为 {BUNDLE_FORMAT}），请更新 zzz-story-extractor",
                bundle.format
            );
        }
        if bundle.story.trim().is_empty() {
            bail!("语料包缺少主线（story 为空）");
        }
        Ok(bundle)
    }

    /// gzip 压缩后的 JSON（测试与工具用）。
    pub fn encode_gzip(&self) -> Result<Vec<u8>> {
        use std::io::Write;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        serde_json::to_writer(&mut encoder, self)?;
        encoder.flush()?;
        Ok(encoder.finish()?)
    }
}

fn collect_md(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("读取 {} 失败", dir.display()))?
    {
        let path = entry?.path();
        if path.is_dir() {
            collect_md(root, &path, out)?;
        } else if path.extension().is_some_and(|e| e == "md") {
            let rel = path
                .strip_prefix(root)
                .map(|r| r.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            let content = std::fs::read_to_string(&path)
                .with_context(|| format!("读取 {} 失败", path.display()))?;
            out.insert(rel, content);
        }
    }
    Ok(())
}
