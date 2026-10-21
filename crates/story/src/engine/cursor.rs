//! Stateless snapshot-bound continuation. Read-only tokens are checksummed, not credentials.
use super::response::NextCall;
use super::{ToolError, err};
use crate::corpus::Corpus;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

const MAX_CURSOR: usize = 16_384;
#[derive(Serialize, Deserialize)]
struct Cursor {
    version: u32,
    revision: String,
    tool: String,
    arguments: Value,
}
pub(crate) fn next(c: &Corpus, tool: &str, mut arguments: Value) -> Result<NextCall, ToolError> {
    if let Some(map) = arguments.as_object_mut() {
        map.retain(|_, v| !v.is_null());
    }
    let payload = serde_json::to_vec(&Cursor {
        version: 1,
        revision: c.revision.clone(),
        tool: tool.into(),
        arguments,
    })
    .map_err(|_| ToolError("无法生成续接参数".into()))?;
    let checksum = Sha256::digest(&payload);
    let mut bytes = Vec::from(&checksum[..16]);
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
    if bytes.len() < 17 {
        return err("cursor 格式错误；请重新查询");
    }
    let (checksum, payload) = bytes.split_at(16);
    if checksum != &Sha256::digest(payload)[..16] {
        return err("cursor 校验失败；请重新查询");
    }
    let cursor: Cursor = serde_json::from_slice(payload)
        .map_err(|_| ToolError("cursor 格式错误；请重新查询".into()))?;
    if cursor.version != 1 || cursor.revision != c.revision {
        return err("语料快照或游标版本已变化；请重新查询获取当前资料");
    }
    if cursor.tool != tool
        || !cursor.arguments.is_object()
        || cursor.arguments.get("cursor").is_some_and(|v| !v.is_null())
    {
        return err("cursor 不适用于此工具；请重新查询");
    }
    Ok(cursor.arguments)
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
    proptest::proptest! {
        #[test] fn roundtrip(value in "[a-zA-Z0-9]{0,100}") {
            let c=crate::corpus::build_from_dir(&std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample")).unwrap();
            let args=serde_json::json!({"keyword":value});let next=next(&c,"ik_search",args.clone()).unwrap();
            proptest::prop_assert_eq!(resolve(&c,"ik_search",next.arguments).unwrap(),args);
        }
    }
}
