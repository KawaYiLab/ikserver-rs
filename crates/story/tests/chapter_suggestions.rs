//! Scope mistakes should provide bounded retry calls without selecting a different scope.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use ik_story::corpus::{Bundle, Corpus, build_from_bundle};
use ik_story::mcp::Server;
use serde_json::{Value, json};

fn corpus() -> Corpus {
    let story = [
        ("Chapter04", "同名章节"),
        ("Chapter040", "同名章节"),
        ("Chapter100", "测试一章"),
        ("Chapter110", "测试二章"),
        ("Chapter120", "测试三章"),
        ("Chapter230", "隐秘往事"),
        ("Chapter240", "天空旅程"),
    ]
    .into_iter()
    .enumerate()
    .map(|(order, (chapter, label))| {
        json!({"chapter":chapter,"chapter_label":label,"family":"GalGame","scene":"1",
            "order":order,"text":"核心资料","speaker":"哲","speaker_status":"resolved"})
        .to_string()
    })
    .collect::<Vec<_>>()
    .join("\n");
    build_from_bundle(&Bundle {
        format: 1,
        manifest: Some(json!({"source_version":"3.3.0"})),
        story,
        supplement: None,
        lore: Default::default(),
    })
    .unwrap()
}

fn rpc(c: &Corpus, tool: &str, args: Value) -> Value {
    let request = json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
        "params":{"name":tool,"arguments":args}});
    serde_json::from_slice(
        &Server::new()
            .unwrap()
            .handle(c, &serde_json::to_vec(&request).unwrap())
            .unwrap(),
    )
    .unwrap()
}

fn error_text(c: &Corpus, tool: &str, args: Value) -> String {
    let response = rpc(c, tool, args);
    assert_eq!(response["result"]["isError"], true, "{response}");
    assert!(response["result"]["structuredContent"].is_null());
    response["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .into()
}

#[test]
fn bare_prefix_has_bounded_chapter_calls_and_remains_an_error() {
    let c = corpus();
    let text = error_text(&c, "ik_catalog", json!({"path":"ch:"}));
    assert!(text.contains("缺少章节"), "{text}");
    assert!(text.contains("候选 7 个，显示 5 个"), "{text}");
    assert_eq!(text.matches("建议重试：").count(), 5, "{text}");
    assert!(text.contains("ik_catalog()"));
    assert_eq!(text, error_text(&c, "ik_catalog", json!({"path":"ch:"})));
}

#[test]
fn misspelled_scope_retry_preserves_all_other_arguments() {
    let c = corpus();
    let text = error_text(
        &c,
        "ik_search",
        json!({"path":"ch:Chaper240","keyword":"核心","speaker":"哲","scope":"story",
            "allowPartial":false,"maxResults":2,"detail":"compact","includeEvidence":true}),
    );
    let expected = "ik_search(includeEvidence=true, allowPartial=false, detail=\"compact\", keyword=\"核心\", maxResults=2, path=\"ch:Chapter240\", scope=\"story\", speaker=\"哲\")";
    assert!(text.contains(expected), "{text}");
    let success = rpc(
        &c,
        "ik_search",
        json!({"path":"ch:Chapter240","keyword":"核心","speaker":"哲","scope":"story",
            "allowPartial":false,"maxResults":2,"detail":"compact","includeEvidence":true}),
    );
    assert_eq!(success["result"]["isError"], false);
}

#[test]
fn ambiguous_name_lists_exact_matches_without_unrelated_chapters() {
    let c = corpus();
    let text = error_text(
        &c,
        "ik_read",
        json!({"docId":"同名章节","limit":2,"includeSources":true}),
    );
    assert!(text.contains("候选 2 个，显示 2 个"), "{text}");
    assert!(text.contains("docId=\"ch:Chapter04\""), "{text}");
    assert!(text.contains("docId=\"ch:Chapter040\""), "{text}");
    assert!(text.contains("limit=2, includeSources=true"), "{text}");
    assert!(!text.contains("ch:Chapter240"), "{text}");
}

#[test]
fn unknown_version_lists_only_reviewed_aliases_and_not_a_guessed_retry() {
    let c = corpus();
    let text = error_text(&c, "ik_catalog", json!({"path":"3.4版本"}));
    assert!(text.contains("已核验版本"), "{text}");
    assert!(text.contains("3.2 → ch:Chapter230"), "{text}");
    assert!(text.contains("3.3 → ch:Chapter240"), "{text}");
    assert!(!text.contains("建议重试："), "{text}");
    let keyword = error_text(&c, "ik_search", json!({"keyword":"3.4版本 核心"}));
    assert!(keyword.contains("已核验版本"), "{keyword}");
    assert!(!keyword.contains("keyword=\"3.3 核心\""), "{keyword}");
}

#[test]
fn valid_scope_and_cursor_errors_are_not_fuzzily_changed() {
    let c = corpus();
    for path in ["ch:Chapter240", "CH:CHAPTER240", "240", "天空旅程", "v3.3"] {
        assert_eq!(
            rpc(&c, "ik_catalog", json!({"path":path}))["result"]["isError"],
            false,
            "{path}"
        );
    }
    let text = error_text(&c, "ik_catalog", json!({"cursor":"broken"}));
    assert!(!text.contains("建议重试："), "{text}");
    let conflict = error_text(&c, "ik_catalog", json!({"cursor":"broken","path":"ch:"}));
    assert!(!conflict.contains("建议重试："), "{conflict}");
}

#[test]
fn missing_chapter_keeps_version_and_doc_filters_in_suggested_scope() {
    let c = corpus();
    let text = error_text(
        &c,
        "ik_search",
        json!({"path":"ch:Chapter241","keyword":"3.3 核心","scope":"story"}),
    );
    assert!(text.contains("候选 1 个，显示 1 个"), "{text}");
    assert!(text.contains("path=\"ch:Chapter240\""), "{text}");
    assert!(text.contains("keyword=\"3.3 核心\""), "{text}");
    assert!(!text.contains("path=\"ch:Chapter230\""), "{text}");

    let doc = c.groups[c.find_group("ch:Chapter230").unwrap()].docs[0] as usize;
    let text = error_text(
        &c,
        "ik_search",
        json!({"path":"ch:","keyword":"核心","docId":c.doc_id(doc)}),
    );
    assert!(text.contains("候选 1 个，显示 1 个"), "{text}");
    assert!(text.contains("path=\"ch:Chapter230\""), "{text}");
    assert!(!text.contains("path=\"ch:Chapter240\""), "{text}");
}

#[test]
fn long_arguments_do_not_expand_retries_or_drop_filters() {
    let c = corpus();
    let long_keyword = "核心".repeat(9000);
    let text = error_text(
        &c,
        "ik_search",
        json!({"path":"ch:","keyword":long_keyword,"speaker":"哲","scope":"story"}),
    );
    assert!(text.contains("参数过长"), "error size: {}", text.len());
    assert!(text.contains("保留筛选条件"), "{text}");
    assert!(!text.contains("建议重试："), "{text}");
    assert!(!text.contains("keyword="), "{text}");
    assert!(text.len() < 4096, "unexpected error size: {}", text.len());
    assert!(text.contains("ik_catalog()"));
}
