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

#[test]
fn cursors_carry_only_non_default_arguments_and_a_short_snapshot_tag() {
    let c = corpus();
    let group = c.groups.iter().find(|g| g.id == "ch:Chapter00").unwrap();
    let ids: Vec<String> = group.docs.iter().map(|&d| c.doc_id(d as usize)).collect();
    let (_, read) = mcp(&c, "ik_read", json!({"docIds":[ids[0], ids[1]],"limit":2}));
    let token = read["nextCall"]["arguments"]["cursor"]
        .as_str()
        .unwrap()
        .to_owned();
    // 旧编码约 326 字符：完整 71 位 revision、长键名、16 字节校验和、空参数 docId=""。
    assert!(token.len() <= 200, "{} {token}", token.len());

    let (_, cat) = mcp(
        &c,
        "ik_catalog",
        json!({"path":"ch:Chapter00","manifest":true}),
    );
    let id = cat["manifest"]["manifestId"].as_str().unwrap().to_owned();
    // 旧编码约 302 字符，其中一半是重复的 revision 与默认值 false。
    assert!(id.len() <= 120, "{} {id}", id.len());

    // 续读照常工作，并读到第二篇。
    let (text, next) = mcp(&c, "ik_read", json!({"cursor": token}));
    assert!(text.contains("第 3-"), "{text}");
    assert!(next["nextCall"].is_object(), "{next}");

    // 语料快照的任何变化（包括 revision 末尾）都让旧游标失效。
    let mut changed = corpus();
    changed.revision.push('x');
    assert!(matches!(
        tools::call_output(&changed, "ik_read", Some(json!({"cursor": token}))),
        Ok(Err(_))
    ));
}

#[test]
fn compact_document_summary_is_one_line_without_unknown_placeholders() {
    let c = corpus();
    let (text, data) = mcp(&c, "ik_catalog", json!({"path":"ch:Chapter00"}));
    // 旧语料里篇序/行序/剧情先后/上下文全是 unknown/未提供，逐篇重复只是噪音。
    assert!(!text.contains("整篇："), "{text}");
    assert!(
        !text.contains("未提供") && !text.contains("=unknown"),
        "{text}"
    );
    // 关键事实保留：署名覆盖、缺口计数、逐句署名名单。
    assert!(text.contains("署名3/6"), "{text}");
    assert!(text.contains("缺署名证据6"), "{text}");
    assert!(text.contains("逐句署名：铃、哲、妮可"), "{text}");
    // 每篇：一行目录 + 一行摘要。
    let docs = data["documents"].as_array().unwrap().len();
    let doc_lines: Vec<&str> = text.lines().filter(|l| l.starts_with("doc_")).collect();
    assert_eq!(doc_lines.len(), docs);
    for (i, line) in text.lines().enumerate() {
        if line.starts_with("doc_") {
            let after: Vec<&str> = text.lines().skip(i + 1).take(2).collect();
            assert!(
                after.len() < 2 || after[1].starts_with("doc_") || !after[0].starts_with("doc_"),
                "{text}"
            );
        }
    }
    assert!(text.lines().count() <= docs * 2 + 6, "{text}");

    // 设定文档不再逐篇写「不适用（设定）」。
    let (lore, _) = mcp(&c, "ik_catalog", json!({"path":"lore:agents"}));
    assert!(!lore.contains("顺序与署名覆盖：不适用"), "{lore}");

    // full 详情不变。
    let (full, _) = mcp(
        &c,
        "ik_catalog",
        json!({"path":"ch:Chapter00","detail":"full"}),
    );
    assert!(full.contains("整篇：篇序="), "{full}");
}

#[test]
fn compact_output_says_once_that_unlisted_order_fields_are_unknown() {
    let c = corpus();
    let note = "未列出即 unknown";
    // 逐篇省略了 unknown 的顺序字段；整份输出说明一次，单篇层面「未知」与「已核验」才可区分。
    for (tool, args) in [
        ("ik_catalog", json!({"path":"ch:Chapter00"})),
        ("ik_search", json!({"keyword":"安比"})),
        ("ik_read", json!({"docId":"ch:Chapter00","limit":3})),
    ] {
        let (text, _) = mcp(&c, tool, args);
        assert_eq!(text.matches(note).count(), 1, "{tool}: {text}");
    }
    // 没有逐篇摘要（清单）、逐篇已全部写出（full）、不适用（设定）时不加。
    for args in [
        json!({"path":"ch:Chapter00","manifest":true}),
        json!({"path":"ch:Chapter00","detail":"full"}),
        json!({"path":"lore:agents"}),
    ] {
        let (text, _) = mcp(&c, "ik_catalog", args.clone());
        assert!(!text.contains(note), "{args}: {text}");
    }
    // 顺序字段全部已知时不加（说明只在确有省略时出现）。
    let mut known = corpus();
    for s in &mut known.doc_summaries {
        s.order.doc_order_source = "source_key".into();
        s.order.chronology_status = "linked".into();
        s.order.context_status = "linked".into();
        for v in s
            .order
            .line_order_sources
            .iter_mut()
            .chain(&mut s.order.line_order_statuses)
        {
            v.value = "source_key".into();
        }
    }
    for (tool, args) in [
        ("ik_catalog", json!({"path":"ch:Chapter00"})),
        ("ik_read", json!({"docId":"ch:Chapter00","limit":3})),
    ] {
        let (text, _) = mcp(&known, tool, args);
        assert!(!text.contains(note), "{tool}: {text}");
    }
}
