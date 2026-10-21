//! Regression cases for compact single-document navigation and actual text budgets.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use ik_story::corpus::{Bundle, Corpus, build_from_bundle};
use ik_story::mcp::tools;
use serde_json::{Value, json};

fn corpus(titles: &[&str], texts: &[&str]) -> Corpus {
    let story = titles
        .iter()
        .enumerate()
        .flat_map(|(scene, title)| {
            texts.iter().enumerate().map(move |(line, text)| {
                json!({
                    "chapter":"Chapter240", "chapter_label":"测试天空章",
                    "family":"GalGame", "scene":scene.to_string(), "scene_title":title,
                    "order":scene*texts.len()+line, "text":text,
                    "speaker":"测试角色", "speaker_status":"resolved", "line_type":"dialogue",
                    "line_order_source":"source_key", "line_order_status":"unknown",
                    "source_key":format!("key_{scene}_{line}"), "line_id":format!("line_{scene}_{line}")
                })
                .to_string()
            })
        })
        .collect::<Vec<_>>()
        .join("\n");
    build_from_bundle(&Bundle {
        format: 1,
        manifest: Some(json!({"source_version":"3.3.0", "accuracy_first":false})),
        story,
        supplement: None,
        lore: Default::default(),
    })
    .unwrap()
}

fn output(c: &Corpus, args: Value) -> (String, Value) {
    let o = tools::call_output(c, "ik_read", Some(args))
        .ok()
        .unwrap()
        .unwrap();
    (
        o.text,
        serde_json::to_value(o.response).unwrap()["data"].clone(),
    )
}

#[test]
fn compact_single_doc_retains_adjacent_navigation_and_preferences() {
    let c = corpus(&["甲篇", "乙篇"], &["完整原文"]);
    let (text, data) = output(
        &c,
        json!({"docId":c.doc_id(0),"limit":7,"maxChars":1200,"lineNumbers":false,"includeSources":true,"includeEvidence":true}),
    );
    assert_eq!(data["complete"], true);
    assert!(data["nextCall"].is_null());
    assert_eq!(data["nextDocument"]["relation"], "catalogAdjacent");
    assert_eq!(data["nextDocument"]["docId"], c.doc_id(1));
    let next = &data["nextDocument"]["nextRead"];
    assert_eq!(next["arguments"]["offset"], 1);
    assert_eq!(next["arguments"]["limit"], 7);
    assert_eq!(next["arguments"]["maxChars"], 1200);
    assert_eq!(next["arguments"]["lineNumbers"], false);
    assert_eq!(next["arguments"]["includeSources"], true);
    assert_eq!(next["arguments"]["includeEvidence"], true);
    assert!(text.contains("下一篇（目录相邻"), "{text}");
    assert!(text.contains("完整返回行"), "{text}");
    let (_, next_data) = output(&c, next["arguments"].clone());
    assert_eq!(next_data["blocks"][0]["document"]["docId"], c.doc_id(1));
    assert!(next_data["nextDocument"].is_null());
}

#[test]
fn compact_chapter_end_and_batch_do_not_offer_unrequested_document() {
    let c = corpus(&["甲篇", "乙篇", "丙篇"], &["完整原文"]);
    let (text, end) = output(&c, json!({"docId":c.doc_id(2)}));
    assert!(text.contains("目录末尾"), "{text}");
    assert!(end["nextDocument"].is_null());
    let (batch_text, batch) = output(&c, json!({"docIds":[c.doc_id(0)]}));
    assert!(batch["nextDocument"].is_null());
    assert!(batch["nextCall"].is_null());
    assert!(!batch_text.contains(&c.doc_id(1)), "{batch_text}");
}

#[test]
fn compact_follow_budget_includes_actual_headers_and_advances_without_empty_blocks() {
    let long_title = "长标题".repeat(80);
    let line = "原文".repeat(45);
    let c = corpus(&["甲篇", &long_title, "丙篇"], &[&line, &line]);
    let mut args = json!({"docId":"ch:Chapter240","maxChars":900});
    let mut seen = Vec::new();
    for _ in 0..10 {
        let (text, data) = output(&c, args);
        let budgeted = text.split("\n\n—").next().unwrap();
        // A single first line may exceed the budget; later lines may not.
        let returned = data["coverageDelta"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["end"].as_u64().unwrap() - r["start"].as_u64().unwrap() + 1)
            .sum::<u64>();
        assert!(budgeted.chars().count() <= 900 || returned == 1, "{text}");
        for b in data["blocks"].as_array().unwrap() {
            assert!(b["start"].as_u64().unwrap() <= b["end"].as_u64().unwrap());
            for l in b["lines"].as_array().unwrap() {
                seen.push((b["document"]["docId"].clone(), l["number"].clone()));
            }
        }
        if data["nextCall"].is_null() {
            break;
        }
        args = data["nextCall"]["arguments"].clone();
    }
    let unique = seen
        .iter()
        .map(|v| serde_json::to_string(v).unwrap())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(seen.len(), 6);
    assert_eq!(unique.len(), 6);
}

#[test]
fn cursor_preserves_original_release_selector_and_read_target_kind() {
    let c = corpus(&["甲篇", "乙篇"], &["第一行", "第二行"]);
    for selector in ["3.3版本", "ch:Chapter240"] {
        let (_, first) = output(&c, json!({"docId":selector,"limit":1}));
        let (_, next) = output(&c, first["nextCall"]["arguments"].clone());
        assert_eq!(next["rangeInfo"], first["rangeInfo"]);
    }
    let (_, first) = output(&c, json!({"docId":c.doc_id(0),"limit":1}));
    let (_, next) = output(&c, first["nextCall"]["arguments"].clone());
    assert_eq!(next["rangeInfo"]["selectionMode"], "document");
    assert_eq!(next["rangeInfo"]["selector"], c.doc_id(0));
}

#[test]
fn cursor_retains_single_document_completion_and_resets_next_target_selector() {
    let c = corpus(&["甲篇", "乙篇"], &["第一行", "第二行"]);
    for selector in ["3.3版本".to_string(), c.doc_id(0)] {
        let (_, first) = output(&c, json!({"docId":selector,"follow":false,"limit":1}));
        let (_, next) = output(&c, first["nextCall"]["arguments"].clone());
        assert_eq!(next["rangeInfo"], first["rangeInfo"]);
        assert_eq!(next["complete"], true);
        assert!(next["nextCall"].is_null());
        assert_eq!(next["blocks"].as_array().unwrap().len(), 1);
        assert_eq!(next["blocks"][0]["document"]["docId"], c.doc_id(0));
        let (_, adjacent) = output(&c, next["nextDocument"]["nextRead"]["arguments"].clone());
        assert_eq!(adjacent["rangeInfo"]["selectionMode"], "document");
        assert_eq!(adjacent["rangeInfo"]["selector"], c.doc_id(1));
    }

    let (_, first) = output(&c, json!({"docId":c.doc_id(0),"follow":true,"limit":2}));
    let (_, next) = output(&c, first["nextCall"]["arguments"].clone());
    assert_eq!(next["rangeInfo"], first["rangeInfo"]);
    assert_eq!(next["blocks"][0]["document"]["docId"], c.doc_id(1));
    assert_eq!(next["complete"], true);
}

#[test]
fn origin_selector_cannot_expand_a_single_document_target() {
    let c = corpus(&["甲篇", "乙篇", "丙篇"], &["完整原文"]);
    for args in [
        json!({"docId":c.doc_id(1),"originSelector":c.doc_id(0),"follow":false}),
        json!({"docId":c.doc_id(1),"originSelector":"3.3版本","follow":false}),
        json!({"docId":c.doc_id(0),"originSelector":c.doc_id(2),"follow":true}),
        json!({"docIds":[c.doc_id(0)],"originSelector":"ch:Chapter240"}),
    ] {
        assert!(
            tools::call_output(&c, "ik_read", Some(args.clone()))
                .ok()
                .unwrap()
                .is_err(),
            "{args}"
        );
    }
}

#[test]
fn origin_selector_rejects_unknown_and_cross_group_context() {
    let c = ik_story::corpus::build_from_dir(
        &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample"),
    )
    .unwrap();
    let other = c
        .docs
        .iter()
        .position(|d| d.group != c.docs[0].group)
        .unwrap();
    for origin in [
        "ch:missing".to_string(),
        c.doc_id(other),
        c.groups[c.docs[other].group as usize].id.clone(),
    ] {
        let args = json!({"docId":c.doc_id(0),"originSelector":origin,"follow":true});
        assert!(
            tools::call_output(&c, "ik_read", Some(args.clone()))
                .ok()
                .unwrap()
                .is_err(),
            "{args}"
        );
    }
}
