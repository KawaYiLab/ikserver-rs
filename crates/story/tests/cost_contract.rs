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

fn coverage_counts(data: &Value) -> Vec<Value> {
    [
        "coveredLines",
        "completeDocuments",
        "partialDocuments",
        "unreadDocuments",
    ]
    .iter()
    .map(|k| data[*k].clone())
    .collect()
}

#[test]
fn coverage_ranges_only_need_doc_id_start_and_end() {
    let c = corpus();
    let (_, cat) = mcp(
        &c,
        "ik_catalog",
        json!({"path":"ch:Chapter00","manifest":true}),
    );
    let id = cat["manifest"]["manifestId"].clone();
    let (_, read) = mcp(&c, "ik_read", json!({"docId":"ch:Chapter00","limit":5}));
    let delta = read["coverageDelta"].clone();
    let (_, full) = mcp(&c, "ik_coverage", json!({"manifestId":id,"ranges":delta}));

    // 只填 docId/start/end：revision 由 manifestId 决定，complete 默认 true。
    let short: Vec<Value> = delta
        .as_array()
        .unwrap()
        .iter()
        .map(|r| json!({"docId":r["docId"],"start":r["start"],"end":r["end"]}))
        .collect();
    let (_, brief) = mcp(&c, "ik_coverage", json!({"manifestId":id,"ranges":short}));
    assert_eq!(coverage_counts(&brief), coverage_counts(&full));
    assert!(brief["coveredLines"].as_u64().unwrap() > 0);

    // 显式传入且不一致的 revision 仍然报错，校验没有放松。
    let mut wrong = short.clone();
    wrong[0]["corpusRevision"] = json!("old");
    assert!(matches!(
        tools::call_output(
            &c,
            "ik_coverage",
            Some(json!({"manifestId":id,"ranges":wrong}))
        ),
        Ok(Err(_))
    ));

    // 工具契约：ranges 每段只要求 docId/start/end。
    let defs = tools::definitions().unwrap();
    let coverage = defs["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "ik_coverage")
        .unwrap()
        .clone();
    let mut required: Vec<String> =
        coverage["inputSchema"]["properties"]["ranges"]["items"]["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
    required.sort();
    assert_eq!(required, ["docId", "end", "start"]);
}
