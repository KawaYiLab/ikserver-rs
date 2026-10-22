//! 常驻开销：清单、coverage 入参、游标、每篇元数据。走 MCP 默认（compact）路径。

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

use serde_json::{Value, json};

use ik_story::corpus::{Corpus, build_from_dir};
use ik_story::mcp::tools;

fn corpus() -> Corpus {
    build_from_dir(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample"))
        .unwrap()
}

/// MCP 调用：返回（文本，structuredContent.data）。
fn mcp(c: &Corpus, tool: &str, args: Value) -> (String, Value) {
    match tools::call_output(c, tool, Some(args)) {
        Ok(Ok(out)) => {
            let response = serde_json::to_value(&out.response).unwrap();
            (out.text, response["data"].clone())
        }
        Ok(Err(e)) => panic!("{tool} failed: {}", e.0),
        Err(_) => panic!("unknown tool {tool}"),
    }
}

#[test]
fn manifest_text_is_a_summary_not_a_replay_of_the_catalog() {
    let c = corpus();
    for detail in ["compact", "full"] {
        let (text, data) = mcp(
            &c,
            "ik_catalog",
            json!({"path":"ch:Chapter00","manifest":true,"detail":detail}),
        );
        // 只给清单摘要：不逐篇列目录，也不重放分组表。
        assert!(!text.contains("ik_read(docId="), "{detail}: {text}");
        assert!(text.lines().count() <= 8, "{detail}: {text}");
        let id = data["manifest"]["manifestId"].as_str().unwrap();
        assert!(text.contains(id), "{text}");
        assert!(text.contains("4 篇 · 12 行"), "{text}");
        // structuredContent 不变：篇目仍在 documents 里。
        assert_eq!(data["documents"].as_array().unwrap().len(), 4);
    }
    let (text, data) = mcp(&c, "ik_catalog", json!({"manifest":true,"maxResults":1}));
    assert!(!text.contains("ik_catalog(path="), "{text}");
    assert!(data["manifest"]["manifestId"].is_string());
}
