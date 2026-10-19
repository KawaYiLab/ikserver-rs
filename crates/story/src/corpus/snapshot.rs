//! 语料快照：预先构建好的语料 + 索引，服务启动时整块读入，免去解析 JSONL 与建索引。
//!
//! 格式：`ZZSC` 魔数 | 格式版本 u32 LE | 负载 xxh3 u64 LE | postcard 负载。

use std::path::Path;

use anyhow::{Context, Result, bail};
use xxhash_rust::xxh3::xxh3_64;

use super::Corpus;

const MAGIC: &[u8; 4] = b"ZZSC";
/// 结构变化时递增，旧快照会被拒绝并提示重建。
const FORMAT_VERSION: u32 = 3;
const HEADER_LEN: usize = 4 + 4 + 8;

pub fn save_snapshot(corpus: &Corpus, path: &Path) -> Result<u64> {
    let payload = postcard::to_stdvec(corpus).context("序列化语料失败")?;
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&xxh3_64(&payload).to_le_bytes());
    out.extend_from_slice(&payload);
    // 先写临时文件再改名，正在运行的服务不会读到写了一半的快照。
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    std::fs::write(&tmp, &out).with_context(|| format!("写入 {} 失败", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("改名为 {} 失败", path.display()))?;
    Ok(out.len() as u64)
}

pub fn load_snapshot(path: &Path) -> Result<Corpus> {
    let bytes = std::fs::read(path).with_context(|| format!("读取快照 {} 失败", path.display()))?;
    if bytes.len() < HEADER_LEN || &bytes[..4] != MAGIC {
        bail!("{} 不是 zzz-story-mcp 快照", path.display());
    }
    let version = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    if version != FORMAT_VERSION {
        bail!(
            "快照格式版本 {version} 与程序（{FORMAT_VERSION}）不一致，请用 `zzz-story-mcp build` 重新构建"
        );
    }
    let mut sum = [0u8; 8];
    sum.copy_from_slice(&bytes[8..16]);
    let payload = &bytes[HEADER_LEN..];
    if xxh3_64(payload) != u64::from_le_bytes(sum) {
        bail!(
            "快照 {} 校验失败（文件损坏或不完整），请重新构建",
            path.display()
        );
    }
    postcard::from_bytes(payload).context("快照反序列化失败")
}
