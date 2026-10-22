//! Stateless snapshot-bound continuation. Read-only tokens are checksummed, not credentials.
use super::response::NextCall;
use super::{ToolError, err};
use crate::corpus::Corpus;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

const MAX_CURSOR: usize = 16_384;
/// 游标格式版本；格式变化时旧游标按「请重新查询」处理（游标只在响应间短期有效，不做迁移）。
const VERSION: u32 = 2;
/// 校验和只防手滑与截断，不是凭据：任何人都可以直接用原参数发起同样的只读查询。
const CHECKSUM_BYTES: usize = 8;
const TOOLS: [&str; 6] = [
    "ik_catalog",
    "ik_search",
    "ik_read",
    "ik_character",
    "ik_guide",
    "ik_coverage",
];
/// 与各工具参数缺省值相同、省略后语义不变的取值。
const DEFAULT_FALSE: [&str; 5] = [
    "excludePlaceholders",
    "excludeUnlinked",
    "includeEvidence",
    "includeSources",
    "manifest",
];
const DEFAULT_ZERO: [&str; 3] = ["offset", "directoryOffset", "batchIndex"];

#[derive(Serialize, Deserialize)]
struct Cursor {
    v: u32,
    /// 语料快照标记：完整 corpusRevision 的 SHA-256 前 8 字节（16 位十六进制）。
    /// 对整串取摘要，revision 任何位置变化都会让旧游标失效。
    r: String,
    t: u8,
    a: Value,
}

fn snapshot_tag(c: &Corpus) -> String {
    Sha256::digest(c.revision.as_bytes())[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// 去掉空值与缺省值，游标只携带真正影响结果的参数。
fn compact_arguments(arguments: &mut Value) {
    if let Some(map) = arguments.as_object_mut() {
        map.retain(|key, v| match v {
            Value::Null => false,
            Value::String(s) => !s.is_empty(),
            Value::Array(a) => !a.is_empty(),
            Value::Bool(false) => !DEFAULT_FALSE.contains(&key.as_str()),
            Value::Number(n) => !(n.as_u64() == Some(0) && DEFAULT_ZERO.contains(&key.as_str())),
            _ => true,
        });
    }
}

pub(crate) fn next(c: &Corpus, tool: &str, mut arguments: Value) -> Result<NextCall, ToolError> {
    compact_arguments(&mut arguments);
    let t = TOOLS
        .iter()
        .position(|&name| name == tool)
        .ok_or_else(|| ToolError("无法生成续接参数".into()))? as u8;
    let payload = serde_json::to_vec(&Cursor {
        v: VERSION,
        r: snapshot_tag(c),
        t,
        a: arguments,
    })
    .map_err(|_| ToolError("无法生成续接参数".into()))?;
    let checksum = Sha256::digest(&payload);
    let mut bytes = Vec::from(&checksum[..CHECKSUM_BYTES]);
    bytes.extend(payload);
    let token = URL_SAFE_NO_PAD.encode(bytes);
    if token.len() > MAX_CURSOR {
        return err("查询参数过长，无法生成游标；请缩短查询后重查");
    }
    Ok(NextCall::new(tool, serde_json::json!({"cursor":token})))
}
pub(crate) fn resolve(c: &Corpus, tool: &str, args: Value) -> Result<Value, ToolError> {
    let Some(token) = args.get("cursor").filter(|v| !v.is_null()) else {
        return Ok(args);
    };
    if args
        .as_object()
        .is_none_or(|a| a.iter().any(|(k, v)| k != "cursor" && !v.is_null()))
    {
        return err("cursor 与查询参数冲突；续接时仅传返回的 cursor，修改条件请重新查询");
    }
    let Some(token) = token.as_str() else {
        return err("cursor 必须是字符串；请重新查询");
    };
    if token.len() > MAX_CURSOR {
        return err("cursor 过长；请重新查询");
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(token)
        .map_err(|_| ToolError("cursor 格式错误；请重新查询".into()))?;
    if bytes.len() <= CHECKSUM_BYTES {
        return err("cursor 格式错误；请重新查询");
    }
    let (checksum, payload) = bytes.split_at(CHECKSUM_BYTES);
    if checksum != &Sha256::digest(payload)[..CHECKSUM_BYTES] {
        return err("cursor 校验失败；请重新查询");
    }
    let cursor: Cursor = serde_json::from_slice(payload)
        .map_err(|_| ToolError("cursor 格式错误；请重新查询".into()))?;
    if cursor.v != VERSION || cursor.r != snapshot_tag(c) {
        return err("语料快照或游标版本已变化；请重新查询获取当前资料");
    }
    if TOOLS.get(cursor.t as usize) != Some(&tool)
        || !cursor.a.is_object()
        || cursor.a.get("cursor").is_some_and(|v| !v.is_null())
    {
        return err("cursor 不适用于此工具；请重新查询");
    }
    Ok(cursor.a)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_and_oversized_are_rejected() {
        let c = crate::corpus::build_from_dir(
            &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample"),
        )
        .unwrap();
        for token in ["!".into(), "x".repeat(MAX_CURSOR + 1)] {
            assert!(resolve(&c, "ik_search", serde_json::json!({"cursor":token})).is_err());
        }
    }
    #[test]
    fn empty_and_default_arguments_are_dropped() {
        let c = crate::corpus::build_from_dir(
            &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample"),
        )
        .unwrap();
        let args = serde_json::json!({
            "docId": "", "docIds": ["doc_x"], "offset": 0, "batchIndex": 0, "limit": 2,
            "includeSources": false, "lineNumbers": false, "follow": null
        });
        let call = next(&c, "ik_read", args).unwrap();
        // lineNumbers 缺省为 true，显式 false 必须保留；其余缺省值省略。
        assert_eq!(
            resolve(&c, "ik_read", call.arguments).unwrap(),
            serde_json::json!({"docIds": ["doc_x"], "limit": 2, "lineNumbers": false})
        );
    }
    proptest::proptest! {
        #[test] fn roundtrip(value in "[a-zA-Z0-9]{1,100}", offset in 1usize..10_000) {
            let c=crate::corpus::build_from_dir(&std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample")).unwrap();
            let args=serde_json::json!({"keyword":value,"offset":offset});let next=next(&c,"ik_search",args.clone()).unwrap();
            proptest::prop_assert_eq!(resolve(&c,"ik_search",next.arguments).unwrap(),args);
        }
    }
}
