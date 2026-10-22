//! Document summaries retain evidence distinctions independently of presentation.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use ik_story::corpus::{Bundle, Corpus, build_from_bundle};
use ik_story::mcp::Server;
use serde_json::{Value, json};

fn corpus() -> Corpus {
    let rows = [
        json!({"speaker":"已解析","speaker_status":"resolved","line_order_source":"source_key","line_order_status":"unknown"}),
        json!({"speaker":"推断","speaker_status":"inferred","line_order_source":"archive_table","line_order_status":"resource_order"}),
        json!({"speaker":"旧署名"}),
        json!({"speaker":"待核验","speaker_status":"future_status","line_order_source":"future_source","line_order_status":"future_order"}),
        json!({"line_type":"unknown","speaker_status":"unresolved","speaker_gap_reason":"no_structured_reference"}),
        json!({"speaker":"原始占位署名","line_type":"placeholder"}),
    ];
    let story = rows
        .into_iter()
        .enumerate()
        .map(|(line, mut row)| {
            row["chapter"] = json!("Chapter240");
            row["family"] = json!("GalGame");
            row["scene"] = json!("0");
            row["order"] = json!(line);
            row["text"] = json!(format!("原文{line}"));
            row["order_source"] = json!("archive_table");
            row["chronology_status"] = json!("unknown");
            row["context_status"] = json!("unlinked");
            row["scene_speakers"] = json!(["姓名表角色"]);
            row.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    build_from_bundle(&Bundle {
        format: 1,
        manifest: Some(json!({"source_version":"3.3.0"})),
        story,
        supplement: None,
        lore: [(
            "agents/test.md".into(),
            "# 档案\n\n## 角色\n设定正文".into(),
        )]
        .into(),
    })
    .unwrap()
}

fn data(c: &Corpus, tool: &str, args: Value) -> Value {
    let body = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":tool,"arguments":args}});
    let bytes = Server::new()
        .unwrap()
        .handle(c, &serde_json::to_vec(&body).unwrap())
        .unwrap();
    let response: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(response["result"]["isError"], false, "{response}");
    response["result"]["structuredContent"]["data"].clone()
}

#[test]
fn scoped_full_catalog_renders_document_summary_once() {
    let c = corpus();
    let result = ik_story::mcp::tools::call_output(
        &c,
        "ik_catalog",
        Some(json!({"path":"ch:Chapter240","scope":"story","detail":"full"})),
    )
    .ok()
    .unwrap()
    .unwrap();
    assert_eq!(result.text.matches("整篇：篇序=").count(), 1);
}

#[test]
fn document_summary_preserves_mixed_order_and_attribution_denominator() {
    let c = corpus();
    let doc = data(&c, "ik_catalog", json!({"path":"ch:Chapter240"}))["documents"][0].clone();
    assert_eq!(doc["order"]["docOrderSource"], "archive_table");
    assert_eq!(doc["order"]["chronologyStatus"], "unknown");
    assert_eq!(doc["order"]["contextStatus"], "unlinked");
    let sources = doc["order"]["lineOrderSources"].as_array().unwrap();
    assert!(
        sources
            .iter()
            .any(|s| s["value"] == "source_key" && s["lineCount"] == 1)
    );
    assert!(
        sources
            .iter()
            .any(|s| s["value"] == "archive_table" && s["lineCount"] == 1)
    );
    assert!(
        sources
            .iter()
            .any(|s| s["value"] == "future_source" && s["lineCount"] == 1)
    );
    assert!(
        sources
            .iter()
            .any(|s| s["value"] == "notProvided" && s["lineCount"] == 3)
    );
    let stats = &doc["speakerCoverage"];
    assert_eq!(stats["totalLines"], 6);
    assert_eq!(stats["namedLines"], 4);
    for key in [
        "resolvedLines",
        "inferredLines",
        "legacyNamedLines",
        "otherNamedLines",
    ] {
        assert_eq!(stats[key], 1);
    }
    assert_eq!(stats["unnamedLines"], 2);
    assert_eq!(doc["speakerSummary"]["names"], json!(["已解析", "旧署名"]));
    assert_eq!(doc["speakerSummary"]["inferredNames"], json!(["推断"]));
    assert_eq!(doc["evidenceSummary"]["placeholders"], 1);
    assert_eq!(doc["evidenceSummary"]["unlinked"], 1);
    assert_eq!(doc["evidenceSummary"]["countsMayOverlap"], true);
}

#[test]
fn lore_summary_marks_order_and_attribution_not_applicable() {
    let c = corpus();
    let doc = data(&c, "ik_catalog", json!({"path":"lore:agents"}))["documents"][0].clone();
    assert_eq!(doc["order"]["applicable"], false);
    assert_eq!(doc["speakerCoverage"]["applicable"], false);
    assert_eq!(doc["speakerCoverage"]["namedRate"], Value::Null);
}

#[test]
fn speaker_preview_is_bounded_and_full_keeps_all_attributed_names() {
    let rows = (0..9).map(|line| json!({"chapter":"Chapter240","family":"GalGame","scene":"0","order":line,"text":"正文","speaker":format!("角色{line}"),"speaker_status":"resolved"}).to_string()).collect::<Vec<_>>().join("\n");
    let c = build_from_bundle(&Bundle {
        format: 1,
        manifest: None,
        story: rows,
        supplement: None,
        lore: Default::default(),
    })
    .unwrap();
    let compact = data(&c, "ik_catalog", json!({"path":"ch:Chapter240"}));
    assert_eq!(
        compact["documents"][0]["speakerSummary"]["names"]
            .as_array()
            .unwrap()
            .len(),
        6
    );
    assert_eq!(
        compact["documents"][0]["speakerSummary"]["namesTruncated"],
        true
    );
    assert_eq!(
        compact["documents"][0]["speakerSummary"]["totalSpeakers"],
        9
    );
    let full = data(
        &c,
        "ik_catalog",
        json!({"path":"ch:Chapter240","detail":"full"}),
    );
    assert_eq!(
        full["documents"][0]["speakerSummary"]["names"]
            .as_array()
            .unwrap()
            .len(),
        9
    );
    // Detail-independent constructor is opt-in; clients can also inspect complete names through full responses.
    let d = c
        .find_doc(compact["documents"][0]["docId"].as_str().unwrap())
        .unwrap();
    let document =
        ik_story::engine::response::Document::new(&c, d).with_summary_detail(&c, d, true);
    assert_eq!(document.speaker_summary.as_ref().unwrap().names.len(), 9);
    assert!(!document.speaker_summary.as_ref().unwrap().names_truncated);
}

#[test]
fn derived_summary_cache_does_not_change_revision_or_cursor() {
    let mut c = corpus();
    let before = data(&c, "ik_catalog", json!({"path":"ch:Chapter240"}));
    let revision = c.revision.clone();
    let read = data(
        &c,
        "ik_read",
        json!({"docId":before["documents"][0]["docId"],"limit":1}),
    );
    c.doc_summaries[0].coverage.legacy_named_lines = 123;
    assert_eq!(c.revision, revision);
    let cached = data(&c, "ik_catalog", json!({"path":"ch:Chapter240"}));
    assert_eq!(
        cached["documents"][0]["docId"],
        before["documents"][0]["docId"]
    );
    let continued = data(&c, "ik_read", read["nextCall"]["arguments"].clone());
    assert_eq!(continued["blocks"][0]["start"], 2);
}

#[test]
fn compact_summary_fits_budget_and_mixed_scope_excludes_lore() {
    let c = corpus();
    let all = (0..c.docs.len()).collect::<Vec<_>>();
    let stats = c.speaker_coverage_for_docs(&all);
    assert_eq!(stats.total_lines, 6);
    assert_eq!(stats.named_lines, 4);
    assert_eq!(stats.excluded_not_applicable_lines, 1);
    let d = c
        .docs
        .iter()
        .position(|d| d.kind == ik_story::corpus::DocKind::Story)
        .unwrap();
    let document = ik_story::engine::response::Document::new(&c, d);
    let mut summary = String::new();
    document.push_summary(&mut summary, false);
    assert!(summary.chars().count() <= 180, "{summary}");
    assert!(summary.contains("archive_table"), "{summary}");
    // 未知的顺序字段不再逐篇写出；已知的（如上下文）照常保留。
    assert!(!summary.contains("剧情先后=unknown"), "{summary}");
    assert!(summary.contains("上下文=unlinked"), "{summary}");
    assert!(summary.contains("未关联1"), "{summary}");
    assert!(summary.contains("署名4/6"), "{summary}");
}

#[test]
fn long_status_summary_keeps_known_order_source_in_budget_fallback() {
    let mut c = corpus();
    let d = c
        .docs
        .iter()
        .position(|d| d.kind == ik_story::corpus::DocKind::Story)
        .unwrap();
    let summary = &mut c.doc_summaries[d];
    summary.order.doc_order_source = "an_exceptionally_long_document_order_source".into();
    summary.order.chronology_status = "an_exceptionally_long_chronology_status".into();
    summary.order.context_status = "an_exceptionally_long_context_status".into();
    summary.evidence.no_voice = 6;
    let text = summary.render_compact(&[], 0);
    assert!(text.contains("source_key"), "{text}");
    assert!(text.contains("混合"), "{text}");
    assert!(text.chars().count() <= 180, "{text}");
    summary.evidence.placeholders = u32::MAX as usize;
    summary.evidence.unknown = u32::MAX as usize;
    summary.evidence.inferred = u32::MAX as usize;
    summary.evidence.missing = u32::MAX as usize;
    summary.evidence.unlinked = u32::MAX as usize;
    summary.evidence.no_voice = u32::MAX as usize;
    let huge = summary.render_compact(&[], 0);
    assert!(huge.contains("source_key"), "{huge}");
    assert!(huge.chars().count() <= 180, "{huge}");
}

#[test]
fn budget_fallback_keeps_document_order_source_names_complete() {
    let mut c = corpus();
    let d = c
        .docs
        .iter()
        .position(|d| d.kind == ik_story::corpus::DocKind::Story)
        .unwrap();
    let summary = &mut c.doc_summaries[d];
    summary.order.chronology_status = "an_exceptionally_long_chronology_status".into();
    summary.order.context_status = "an_exceptionally_long_context_status".into();
    summary.evidence.placeholders = u32::MAX as usize;
    summary.evidence.unknown = u32::MAX as usize;
    summary.evidence.inferred = u32::MAX as usize;
    summary.evidence.missing = u32::MAX as usize;
    summary.evidence.unlinked = u32::MAX as usize;
    summary.evidence.no_voice = u32::MAX as usize;
    for source in ["source_key", "archive_table"] {
        summary.order.doc_order_source = source.into();
        let text = summary.render_compact(&[], 0);
        assert!(text.contains(&format!("篇序={source} ")), "{text}");
        assert!(text.contains("未关联"), "{text}");
        assert!(text.contains("缺证据"), "{text}");
        assert!(text.contains("未知"), "{text}");
        assert!(text.chars().count() <= 180, "{text}");
    }
}

#[test]
fn budget_fallback_with_large_coverage_preserves_gap_markers() {
    let mut c = corpus();
    let d = c
        .docs
        .iter()
        .position(|d| d.kind == ik_story::corpus::DocKind::Story)
        .unwrap();
    let summary = &mut c.doc_summaries[d];
    summary.order.doc_order_source = "archive_table".into();
    summary.order.chronology_status = "an_exceptionally_long_chronology_status".into();
    summary.order.context_status = "an_exceptionally_long_context_status".into();
    summary.evidence.placeholders = usize::MAX;
    summary.evidence.unknown = usize::MAX;
    summary.evidence.inferred = usize::MAX;
    summary.evidence.missing = usize::MAX;
    summary.evidence.unlinked = usize::MAX;
    summary.evidence.no_voice = usize::MAX;
    summary.coverage.named_lines = usize::MAX;
    summary.coverage.total_lines = usize::MAX;
    summary.coverage.inferred_lines = usize::MAX;
    summary.coverage.legacy_named_lines = usize::MAX;
    summary.coverage.other_named_lines = usize::MAX;
    let text = summary.render_compact(&[], 0);
    for marker in [
        "篇序=archive_table",
        "source_key",
        "混合",
        "占位",
        "未知",
        "推断",
        "缺证据",
        "未关联",
        "无配音",
        "署名",
        "full",
    ] {
        assert!(text.contains(marker), "{text}");
    }
    assert!(text.chars().count() <= 180, "{text}");
}
