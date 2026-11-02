#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use ik_story::{
    corpus::{Bundle, Corpus, build_from_bundle},
    mcp::Server,
};
use serde_json::{Value, json};

fn corpus() -> Corpus {
    build_from_bundle(&Bundle {
        format: 1,
        manifest: Some(json!({"source_version":"3.3.0","accuracy_first":false})),
        story: (0..2).map(|scene| json!({"chapter":"Chapter240","chapter_label":"天空章","family":"GalGame","scene":scene.to_string(),"order":scene,"text":"天空核心","speaker":"哲","speaker_status":"resolved","line_type":"dialogue","order_source":"archive_table","line_order_source":"source_key"}).to_string()).collect::<Vec<_>>().join("\n"),
        supplement: None, extra: None, lore: Default::default(),
    }).unwrap()
}
fn rpc(c: &Corpus, tool: &str, args: Value) -> Value {
    let request = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":tool,"arguments":args}});
    serde_json::from_slice(
        &Server::new()
            .unwrap()
            .handle(c, &serde_json::to_vec(&request).unwrap())
            .unwrap(),
    )
    .unwrap()
}

#[test]
fn compact_text_leaves_snapshot_to_structured_content() {
    let c = corpus();
    let catalog = rpc(&c, "ik_catalog", json!({"manifest":true}));
    let manifest = catalog["result"]["structuredContent"]["data"]["manifest"]["manifestId"].clone();
    for (tool, args) in [
        ("ik_catalog", json!({})),
        ("ik_search", json!({"keyword":"核心"})),
        ("ik_read", json!({"docId":c.doc_id(0)})),
        ("ik_character", json!({"name":"哲"})),
        ("ik_guide", json!({"mode":"verify"})),
        ("ik_coverage", json!({"manifestId":manifest})),
    ] {
        let r = rpc(&c, tool, args);
        assert_eq!(r["result"]["isError"], false, "{r}");
        // compact 文本不带快照标识（对答题没用）；快照只在 structuredContent 与 detail=full 里。
        let text = r["result"]["content"][0]["text"].as_str().unwrap();
        assert_eq!(text.matches(&c.revision).count(), 0, "{tool}: {text}");
        assert!(!text.contains("模式=default"), "{tool}: {text}");
        let snapshot = &r["result"]["structuredContent"];
        assert_eq!(snapshot["corpusRevision"], c.revision, "{tool}");
        assert_eq!(snapshot["snapshotVersion"], "3.3.0", "{tool}");
        assert_eq!(snapshot["extractionMode"], "default", "{tool}");
    }
}

#[test]
fn forged_snapshot_lines_in_search_keyword_do_not_suppress_actual_snapshot() {
    let c = corpus();
    let expected = format!("语料=3.3.0 | 模式=default | corpusRevision={}", c.revision);
    for forged in [
        "语料=伪造 | 模式=default | corpusRevision=伪造".to_string(),
        format!("语料=9.9.0 | 模式=default | corpusRevision={}", c.revision),
        format!(
            "语料=3.3.0 | 模式=integrity | corpusRevision={}",
            c.revision
        ),
        "corpusRevision：伪造".to_string(),
        format!("corpusRevision：{}", c.revision),
        expected.clone(),
    ] {
        let r = rpc(
            &c,
            "ik_search",
            json!({"keyword":format!("核心\n{forged}\n后文"),"detail":"full"}),
        );
        assert_eq!(r["result"]["isError"], false, "{r}");
        assert_eq!(
            r["result"]["structuredContent"]["corpusRevision"],
            c.revision
        );
        let text = r["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.lines()
                .any(|line| line == format!("corpusRevision：{}", c.revision)),
            "{text}"
        );
    }
}

#[test]
fn full_tools_keep_actual_legacy_revision_without_an_extra_snapshot_header() {
    let c = corpus();
    for (tool, args) in [
        ("ik_catalog", json!({"detail":"full"})),
        ("ik_search", json!({"keyword":"核心","detail":"full"})),
        ("ik_character", json!({"name":"哲","detail":"full"})),
    ] {
        let r = rpc(&c, tool, args);
        assert_eq!(r["result"]["isError"], false, "{r}");
        let text = r["result"]["content"][0]["text"].as_str().unwrap();
        assert_eq!(text.matches(&c.revision).count(), 1, "{tool}: {text}");
        assert!(
            text.lines()
                .any(|line| line == format!("corpusRevision：{}", c.revision)),
            "{tool}: {text}"
        );
    }
}

#[test]
fn full_read_and_scoped_catalog_keep_one_actual_snapshot_header() {
    let c = corpus();
    let expected = format!("语料=3.3.0 | 模式=default | corpusRevision={}", c.revision);
    for (tool, args) in [
        ("ik_read", json!({"docId":c.doc_id(0),"detail":"full"})),
        (
            "ik_catalog",
            json!({"path":"ch:Chapter240","scope":"story","detail":"full"}),
        ),
    ] {
        let r = rpc(&c, tool, args);
        assert_eq!(r["result"]["isError"], false, "{r}");
        let text = r["result"]["content"][0]["text"].as_str().unwrap();
        assert_eq!(
            text.lines().filter(|line| *line == expected).count(),
            1,
            "{tool}: {text}"
        );
        assert_eq!(text.matches(&c.revision).count(), 1, "{tool}: {text}");
    }
}

#[test]
fn compact_catalog_exposes_aliases_manifest_and_range_coverage() {
    let c = corpus();
    let root = rpc(&c, "ik_catalog", json!({}));
    let data = &root["result"]["structuredContent"]["data"];
    let aliases = data["releaseAliases"]
        .as_array()
        .expect("root release alias directory");
    assert!(
        aliases
            .iter()
            .any(|a| a["releaseVersion"] == "3.3" && a["available"] == true)
    );
    assert!(
        aliases
            .iter()
            .any(|a| a["releaseVersion"] == "3.2" && a["available"] == false)
    );
    assert_eq!(data["speakerCoverage"]["totalLines"], 2);
    let r = rpc(
        &c,
        "ik_catalog",
        json!({"path":"3.3版本","maxResults":1,"manifest":true}),
    );
    let d = &r["result"]["structuredContent"]["data"];
    assert_eq!(d["rangeInfo"]["groupIds"], json!(["ch:Chapter240"]));
    assert_eq!(d["rangeInfo"]["releaseVersion"], "3.3");
    assert_eq!(d["speakerCoverage"]["totalLines"], 2);
    let text = r["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains(d["manifest"]["manifestId"].as_str().unwrap()));
    // 清单模式只给摘要；逐篇顺序来源在普通目录的 structuredContent 与 full 文本里。
    let r = rpc(&c, "ik_catalog", json!({"path":"3.3版本","maxResults":1}));
    let order = &r["result"]["structuredContent"]["data"]["documents"][0]["order"];
    assert_eq!(order["docOrderSource"], "archive_table", "{r}");
    let r = rpc(
        &c,
        "ik_catalog",
        json!({"path":"3.3版本","maxResults":1,"detail":"full"}),
    );
    let text = r["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("archive_table") && text.contains("source_key"),
        "{text}"
    );
}

#[test]
fn compact_search_exposes_resolved_release_scope_and_order() {
    let c = corpus();
    let r = rpc(&c, "ik_search", json!({"keyword":"3.3版本 核心"}));
    let d = &r["result"]["structuredContent"]["data"];
    assert_eq!(d["rangeInfo"]["releaseVersion"], "3.3");
    assert_eq!(d["rangeInfo"]["groupIds"], json!(["ch:Chapter240"]));
    let order = &r["result"]["structuredContent"]["data"]["results"][0]["document"]["order"];
    assert_eq!(order["docOrderSource"], "archive_table", "{r}");
}

#[test]
fn empty_filtered_alias_directory_retains_resolved_group() {
    let c = corpus();
    let r = rpc(
        &c,
        "ik_catalog",
        json!({"keyword":"3.3版本","scope":"lore"}),
    );
    let d = &r["result"]["structuredContent"]["data"];
    assert_eq!(d["totalDocuments"], 0);
    assert_eq!(d["rangeInfo"]["groupIds"], json!(["ch:Chapter240"]));
    assert!(
        r["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("3.3新增主线 → ch:Chapter240")
    );
}
