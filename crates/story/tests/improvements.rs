//! M0–M2 regression fixtures use invented text; real corpus reports stay outside Git.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use ik_story::corpus::{Bundle, Corpus, build_from_bundle};
use ik_story::mcp::tools;
use serde_json::{Value, json};

fn sample() -> Bundle {
    let rows = [
        json!({"scene":"32", "speaker":"安比", "line_type":"placeholder", "text":"场景占位说明", "content_status":"reviewed_placeholder", "content_evidence":{"reason":"scene_outline_in_dialogue_node","playback_status":"unverified"}, "source_attribution":{"speaker":"安比"}, "speaker_status":"not_applicable", "has_voice":true, "perform":{"assets":["Perform/32.asset"],"occurrences":[{"asset":"Perform/32.asset","section":0,"node":1}]}}),
        json!({"scene":"36", "text":"候选铃称呼", "scene_speakers":["铃","哲","Fairy"], "speaker_status":"unresolved", "speaker_gap_reason":"no_structured_reference", "has_voice":false}),
        json!({"scene":"36", "text":"候选铃称呼", "scene_speakers":["铃","哲","Fairy"], "speaker_status":"unresolved", "has_voice":false}),
        json!({"scene":"36", "text":"候选哲称呼", "speaker_status":"future_status", "speaker_source":"future_source"}),
        json!({"scene":"1", "text":"天空在这里", "speaker":"服装甲", "speaker_id":"角色甲", "speaker_status":"resolved", "speaker_source":"perform", "has_voice":false}),
        json!({"scene":"1", "text":"核心在这里", "speaker":"???", "speaker_status":"resolved", "speaker_source":"perform"}),
        json!({"scene":"2", "text":"只有天空", "speaker":"哲", "speaker_status":"inferred", "speaker_source":"scene_name"}),
        json!({"scene":"3", "text":"无关的原文", "scene_title":"独有标题", "line_type":"unknown"}),
    ];
    let story = rows
        .into_iter()
        .enumerate()
        .map(|(i, mut r)| {
            r["chapter"] = json!("Chapter240");
            r["chapter_label"] = json!("重返天空的旅程");
            r["family"] = json!("GalGame");
            r["order"] = json!(i);
            r["source_key"] = json!(format!("synthetic_key_{i}"));
            r["line_id"] = json!(format!("synthetic_line_{i}"));
            r["line_order_source"] = json!("source_key");
            r["line_order_status"] = json!("unknown");
            r.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    Bundle {
        format: 1,
        manifest: Some(json!({"source_version":"3.3.0","language":"CHS","accuracy_first":false})),
        story,
        supplement: None,
        extra: None,
        lore: Default::default(),
    }
}

fn call(c: &Corpus, name: &str, args: Value) -> String {
    tools::call(c, name, Some(args)).ok().unwrap().unwrap()
}

#[test]
fn evidence_placeholder_is_not_dialogue_and_name_table_is_not_appearance() {
    let c = build_from_bundle(&sample()).unwrap();
    let out = call(
        &c,
        "ik_read",
        json!({"docId":c.doc_id(0),"includeEvidence":true,"includeSources":true}),
    );
    assert!(out.contains("【占位说明】场景占位说明"), "{out}");
    assert!(!out.contains("安比：场景占位说明"), "{out}");
    assert!(
        out.contains("Perform/32.asset")
            && out.contains("source_attribution")
            && out.contains("unverified"),
        "{out}"
    );
    let out = call(&c, "ik_character", json!({"name":"Fairy"}));
    assert!(out.contains("出场 0 场景 · 台词 0 句"), "{out}");
    assert!(out.contains("姓名表关联 1 场景"), "{out}");
    assert!(!out.contains("| 目录首见出场"), "{out}");
}

#[test]
fn classified_search_keeps_metadata_out_of_body_quota_and_supports_body_and() {
    let c = build_from_bundle(&sample()).unwrap();
    let out = call(
        &c,
        "ik_search",
        json!({"keyword":"天空 核心 秘密","maxResults":1,"allowPartial":true}),
    );
    assert!(
        out.contains("正文结果 2 篇") && out.contains("目录建议 3 篇"),
        "{out}"
    );
    assert!(
        out.contains("天空在这里") && out.contains("核心在这里"),
        "{out}"
    );
    assert!(
        out.contains("正文匹配词：天空、核心") && out.contains("全库无此词：秘密"),
        "{out}"
    );
    let out = call(
        &c,
        "ik_search",
        json!({"keyword":"独有标题","matchIn":"body","allowPartial":false}),
    );
    assert!(out.contains("命中 0 篇"), "{out}");
    let out = call(&c, "ik_search", json!({"keyword":"独有标题"}));
    assert!(
        out.contains("目录建议 1 篇") && out.contains("原文入口：ik_read"),
        "{out}"
    );
    let out = call(&c, "ik_search", json!({"keyword":"Fairy"}));
    assert!(
        out.contains("姓名表") && out.contains("正文结果 0 篇"),
        "{out}"
    );
}

#[test]
fn revision_and_line_identity_survive_gzip_reload_and_timestamp_changes() {
    let b = sample();
    let a = build_from_bundle(&b).unwrap();
    let gz = b.encode_gzip().unwrap();
    let mut decoded = Bundle::decode(&gz, 1_000_000).unwrap();
    decoded.manifest.as_mut().unwrap()["generated_at"] = json!("changed build time");
    decoded.story = decoded
        .story
        .lines()
        .map(|l| {
            serde_json::to_string_pretty(&serde_json::from_str::<Value>(l).unwrap())
                .unwrap()
                .replace('\n', " ")
        })
        .collect::<Vec<_>>()
        .join("\r\n");
    let reloaded = build_from_bundle(&decoded).unwrap();
    assert_eq!(a.revision, reloaded.revision);
    assert_eq!(a.doc_id(0), reloaded.doc_id(0));
    assert_eq!(a.line_sources[0].line_id, reloaded.line_sources[0].line_id);
    assert_eq!(a.line_evidence(0), reloaded.line_evidence(0));
    decoded.manifest.as_mut().unwrap()["accuracy_first"] = json!(true);
    assert_ne!(a.revision, build_from_bundle(&decoded).unwrap().revision);
    decoded.manifest.as_mut().unwrap()["accuracy_first"] = json!(false);
    decoded.story = decoded.story.replace("场景占位说明", "另一个占位说明");
    assert_ne!(a.revision, build_from_bundle(&decoded).unwrap().revision);
}

#[test]
fn unknown_evidence_and_duplicate_candidates_survive_without_rewriting_names() {
    let c = build_from_bundle(&sample()).unwrap();
    let out = call(
        &c,
        "ik_read",
        json!({"docId":c.doc_id(1),"includeEvidence":"true"}),
    );
    assert_eq!(out.matches("【署名未知】候选铃称呼").count(), 2, "{out}");
    assert!(
        out.contains("future_status") && out.contains("future_source"),
        "{out}"
    );
    assert!(out.contains("候选哲称呼"), "{out}");
    let out = call(
        &c,
        "ik_read",
        json!({"docId":c.doc_id(2),"includeEvidence":true}),
    );
    assert!(
        out.contains("服装甲：天空") && out.contains("???：核心"),
        "{out}"
    );
    let out = call(&c, "ik_character", json!({"name":"哲"}));
    assert!(
        out.contains("出场 0 场景 · 台词 0 句") && out.contains("推断署名 1 句"),
        "{out}"
    );
}

#[test]
fn match_fields_body_only_and_filters_are_consistent_globally_and_within_doc() {
    let c = build_from_bundle(&sample()).unwrap();
    let doc = c.doc_id(2);
    let out = call(
        &c,
        "ik_search",
        json!({"keyword":"天空 核心","matchIn":"body","allowPartial":"false","docId":doc}),
    );
    assert!(
        out.contains("命中 2 行") && out.contains("正文匹配词：天空、核心"),
        "{out}"
    );
    let out = call(
        &c,
        "ik_search",
        json!({"keyword":"天空 核心","matchIn":"body","allowPartial":false,"speaker":"角色甲"}),
    );
    assert!(out.contains("命中 0 篇"), "{out}");
    for args in [
        json!({"keyword":"天空 核心","docId":doc,"scope":"lore"}),
        json!({"keyword":"天空 核心","path":"3.3版本","scope":"supplement"}),
    ] {
        let out = call(&c, "ik_search", args);
        assert!(
            !out.contains("天空在这里") && !out.contains("核心在这里"),
            "{out}"
        );
    }
    let out = call(
        &c,
        "ik_search",
        json!({"keyword":"独有标题 无关","allowPartial":false}),
    );
    assert!(
        out.contains("正文结果 1 篇")
            && out.contains("独有标题：标题")
            && out.contains("正文缺失词：独有标题"),
        "{out}"
    );
    let out = call(
        &c,
        "ik_search",
        json!({"keyword":"角色甲","allowPartial":false}),
    );
    assert!(
        out.contains("角色甲：角色身份") && out.contains("服装甲：天空"),
        "{out}"
    );
    assert!(
        tools::call(
            &c,
            "ik_search",
            Some(json!({"keyword":"天空","matchIn":"invalid"}))
        )
        .ok()
        .unwrap()
        .is_err()
    );
}

#[test]
fn directory_pages_preserve_scope_and_do_not_repeat_entries() {
    let c = build_from_bundle(&sample()).unwrap();
    let mut seen = std::collections::HashSet::new();
    let mut args = json!({"keyword":"天空 核心 秘密","path":"3.3版本","scope":"story","maxDirectoryResults":1,"allowPartial":true});
    loop {
        let output = tools::call_output(&c, "ik_search", Some(args))
            .ok()
            .unwrap()
            .unwrap();
        let value = serde_json::to_value(output.response).unwrap();
        let data = &value["data"];
        for item in data["directorySuggestions"].as_array().unwrap() {
            assert!(seen.insert(item["document"]["docId"].as_str().unwrap().to_owned()));
        }
        assert_eq!(data["resolvedRange"]["groupId"], "ch:Chapter240");
        assert_eq!(data["resolvedRange"]["scope"], "story");
        if data["nextDirectoryCall"].is_null() {
            break;
        }
        args = data["nextDirectoryCall"]["arguments"].clone();
    }
    assert_eq!(seen.len(), 3);
}

#[test]
fn evidence_tool_definitions_and_jsonrpc_keep_text_compatibility() {
    let c = build_from_bundle(&sample()).unwrap();
    let defs = tools::definitions().unwrap();
    for tool in ["ik_catalog", "ik_search", "ik_read", "ik_character"] {
        let definition = defs["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["name"] == tool)
            .unwrap();
        assert_eq!(
            definition["inputSchema"]["properties"]["includeEvidence"]["type"],
            "boolean"
        );
    }
    let server = ik_story::mcp::Server::new().unwrap();
    let request = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"ik_read","arguments":{"docId":c.doc_id(0),"includeEvidence":true}}});
    let response = server
        .handle(&c, &serde_json::to_vec(&request).unwrap())
        .unwrap();
    let response: Value = serde_json::from_slice(&response).unwrap();
    assert_eq!(response["result"]["content"][0]["type"], "text");
    assert!(
        response["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("【占位说明】")
    );
}

#[test]
fn evidence_expansion_is_bounded_and_read_continuation_keeps_preferences() {
    let c = build_from_bundle(&sample()).unwrap();
    let out = call(
        &c,
        "ik_read",
        json!({"docId":c.doc_id(1),"limit":1,"includeEvidence":"true","lineNumbers":false}),
    );
    let next = out.split("继续：").nth(1).unwrap();
    assert!(
        next.contains("includeEvidence=true")
            && next.contains("limit=1")
            && next.contains("lineNumbers=false"),
        "{out}"
    );
    let out = call(
        &c,
        "ik_catalog",
        json!({"path":"3.3版本","includeEvidence":true}),
    );
    assert!(
        out.contains("Perform/32.asset") && out.contains("future_status"),
        "{out}"
    );
    let out = call(
        &c,
        "ik_character",
        json!({"name":"角色甲","includeEvidence":true}),
    );
    assert!(
        out.contains("逐句署名证据入口") && out.contains("speaker_status"),
        "{out}"
    );
    let mut b = sample();
    let row: Value = serde_json::from_str(b.story.lines().next().unwrap()).unwrap();
    b.story = (0..30)
        .map(|i| {
            let mut row = row.clone();
            row["line_id"] = json!(format!("bounded_{i}"));
            row["order"] = json!(i);
            row.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    let c = build_from_bundle(&b).unwrap();
    let out = call(
        &c,
        "ik_catalog",
        json!({"path":c.doc_id(0),"includeEvidence":true}),
    );
    assert!(
        out.contains("bounded_19") && !out.contains("bounded_20") && out.contains("offset=21"),
        "{out}"
    );
}

#[tokio::test]
async fn http_tools_call_accepts_evidence_and_body_only_search() {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let c = build_from_bundle(&sample()).unwrap();
    let config = ik_kernel::config::Story {
        public: true,
        ..Default::default()
    };
    let state = ik_story::StoryState::with_corpus(c, &config).unwrap();
    let (router, _) = ik_story::mcp_router().split_for_parts();
    let app = router.with_state(state);
    for args in [
        json!({"keyword":"天空 核心","matchIn":"body","allowPartial":false,"includeEvidence":"true"}),
        json!({"keyword":"独有标题","matchIn":"body","allowPartial":false}),
    ] {
        let request = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"ik_search","arguments":args}});
        let response = app
            .clone()
            .oneshot(
                Request::post("/v1/story/mcp")
                    .header("content-type", "application/json")
                    .header("accept", "application/json, text/event-stream")
                    .body(Body::from(request.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["result"]["isError"], false);
        let text = value["result"]["content"][0]["text"].as_str().unwrap();
        if args["keyword"] == "独有标题" {
            assert!(text.contains("命中 0 篇"), "{text}");
        } else {
            assert!(
                text.contains("正文结果 1 篇") && text.contains("speaker_status"),
                "{text}"
            );
        }
    }
}

#[test]
#[ignore = "requires STORY_BASELINE_SOURCE and STORY_BASELINE_REPORT; no database"]
fn real_corpus_baseline() {
    use sha2::{Digest, Sha256};
    use std::time::Instant;
    let source = std::env::var("STORY_BASELINE_SOURCE").unwrap();
    let destination = std::env::var("STORY_BASELINE_REPORT").unwrap();
    let bundle = Bundle::from_dir(std::path::Path::new(&source)).unwrap();
    let digest = Sha256::digest(serde_json::to_vec(&bundle).unwrap())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let start = Instant::now();
    let c = build_from_bundle(&bundle).unwrap();
    let build_ms = start.elapsed().as_secs_f64() * 1000.0;
    let queries = [
        (
            "ik_search",
            json!({"keyword":"天空 核心 秘密","path":"3.3版本"}),
        ),
        ("ik_search", json!({"keyword":"3.3版本 核心"})),
        (
            "ik_search",
            json!({"keyword":"天空","path":"3.3版本","allowPartial":false}),
        ),
        ("ik_search", json!({"keyword":"Fairy","path":"3.3版本"})),
        ("ik_search", json!({"keyword":"核","path":"3.3版本"})),
        ("ik_catalog", json!({"path":"3.3版本"})),
        ("ik_read", json!({"docId":"3.3版本","limit":20})),
    ];
    let mut results = Vec::new();
    for (tool, args) in queries {
        let mut latency = Vec::new();
        let mut output = String::new();
        for _ in 0..25 {
            let start = Instant::now();
            output = call(&c, tool, args.clone());
            latency.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        latency.sort_by(f64::total_cmp);
        results.push(json!({"tool":tool,"args":args,"p50_ms":latency[12],"p95_ms":latency[23],"response_bytes":output.len(),"output":output}));
    }
    let mut special = Vec::new();
    for (i, d) in c.docs.iter().enumerate().filter(|(_, d)| {
        d.kind == ik_story::corpus::DocKind::Story
            && d.lines().any(|l| {
                c.line_sources[l].source_key.as_deref().is_some_and(|k| {
                    ["032", "033", "034", "036", "037"]
                        .iter()
                        .any(|a| k.contains(&format!("124000{a}_")))
                })
            })
    }) {
        if c.groups[d.group as usize].id != "ch:Chapter240" {
            continue;
        }
        special.push(json!({"docId":c.doc_id(i),"title":d.title,"lines":d.line_count,"read":call(&c,"ik_read",json!({"docId":c.doc_id(i),"includeEvidence":true,"includeSources":true}))}));
    }
    let memory = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!("(Get-Process -Id {}).WorkingSet64", std::process::id()),
        ])
        .output()
        .unwrap();
    let working_set_bytes = String::from_utf8_lossy(&memory.stdout)
        .trim()
        .parse::<u64>()
        .unwrap();
    let report = json!({"source":source,"bundle_sha256":digest,"corpus_revision":c.revision,"manifest":bundle.manifest,"documents":c.docs.len(),"lines":c.lines.len(),"text_bytes":c.text.len(),"build_ms":build_ms,"working_set_bytes":working_set_bytes,"queries":results,"special_scenes":special});
    std::fs::write(destination, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
}
