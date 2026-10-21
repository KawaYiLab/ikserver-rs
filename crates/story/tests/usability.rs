//! Model-facing contracts: no game text or database required.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use ik_story::corpus::{Bundle, Corpus, build_from_bundle};
use ik_story::mcp::Server;
use serde_json::{Value, json};

fn corpus() -> Corpus {
    let story = (0..7).flat_map(|scene| (0..5).map(move |line| json!({
        "chapter":"Chapter240","chapter_label":"测试天空章","family":"GalGame",
        "scene":scene.to_string(),"scene_title":format!("测试场景{scene}"),"order":scene*5+line,
        "text":match line {0=>"天空问题",1=>"测试邻行",2=>"核心资料",_=>"重复原文"},
        "line_type":"unknown","speaker_status":"unresolved","speaker_gap_reason":"no_structured_reference",
        "source_key":format!("key_{scene}_{line}"),"line_id":format!("line_{scene}_{line}"),
        "line_order_source":"source_key","line_order_status":"unknown"
    }))).map(|v|v.to_string()).collect::<Vec<_>>().join("\n");
    build_from_bundle(&Bundle {
        format: 1,
        manifest: Some(json!({"source_version":"3.3.0","accuracy_first":false})),
        story,
        supplement: None,
        lore: Default::default(),
    })
    .unwrap()
}

fn rpc(c: &Corpus, tool: &str, args: Value) -> Value {
    let body = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":tool,"arguments":args}});
    serde_json::from_slice(
        &Server::new()
            .unwrap()
            .handle(c, &serde_json::to_vec(&body).unwrap())
            .unwrap(),
    )
    .unwrap()
}

#[test]
fn search_structured_pages_can_continue_with_only_the_returned_call() {
    let c = corpus();
    let first = rpc(
        &c,
        "ik_search",
        json!({"keyword":"天空 核心","path":"3.3版本","maxResults":2}),
    );
    let mut result = first["result"].clone();
    assert!(result["structuredContent"].is_object(), "{result}");
    let mut seen = std::collections::HashSet::new();
    loop {
        let data = &result["structuredContent"]["data"];
        for doc in data["results"].as_array().unwrap() {
            assert!(seen.insert(doc["document"]["docId"].as_str().unwrap().to_owned()));
        }
        if data["nextCall"].is_null() {
            break;
        }
        let next = &data["nextCall"];
        assert_eq!(next["tool"], "ik_search");
        assert_eq!(next["arguments"].as_object().unwrap().len(), 1);
        result = rpc(&c, "ik_search", next["arguments"].clone())["result"].clone();
        assert_eq!(result["isError"], false);
    }
    assert_eq!(seen.len(), 7);
}

#[test]
fn read_returns_exact_coverage_and_preserves_next_call_preferences() {
    let c = corpus();
    let response = rpc(
        &c,
        "ik_read",
        json!({"docId":"3.3版本","limit":2,"lineNumbers":false,"includeEvidence":true}),
    );
    let data = &response["result"]["structuredContent"]["data"];
    assert_eq!(data["coverageDelta"][0]["start"], 1);
    assert_eq!(data["coverageDelta"][0]["end"], 2);
    let next = rpc(&c, "ik_read", data["nextCall"]["arguments"].clone());
    assert_eq!(
        next["result"]["structuredContent"]["data"]["coverageDelta"][0]["start"],
        3
    );
}

fn data(c: &Corpus, tool: &str, args: Value) -> Value {
    let response = rpc(c, tool, args);
    assert_eq!(response["result"]["isError"], false, "{response}");
    response["result"]["structuredContent"]["data"].clone()
}

#[test]
fn window_is_bounded_and_does_not_use_metadata_for_coverage() {
    let c = corpus();
    let strict = data(
        &c,
        "ik_search",
        json!({"keyword":"天空 核心","matchUnit":"window","windowLines":2,"allowPartial":false}),
    );
    assert_eq!(strict["candidateCount"], 0);
    let complete = data(
        &c,
        "ik_search",
        json!({"keyword":"天空 核心","matchUnit":"window","windowLines":3,"contextLines":0,"allowPartial":false}),
    );
    assert_eq!(complete["candidateCount"], 7);
    assert_eq!(complete["results"][0]["windows"][0]["start"], 1);
    assert_eq!(complete["results"][0]["windows"][0]["end"], 3);
    assert_eq!(
        complete["results"][0]["windows"][0]["orderStatus"],
        "playback_unverified"
    );
    let partial = data(
        &c,
        "ik_search",
        json!({"keyword":"天空 核心","matchUnit":"window","windowLines":2}),
    );
    assert_eq!(partial["partial"], true);
    assert_eq!(partial["results"][0]["missingTerms"], json!(["核心"]));
}

#[test]
fn window_pages_do_not_repeat_matched_rows() {
    let c = corpus();
    let id = c.doc_id(0);
    let mut args =
        json!({"keyword":"重复","docId":id,"matchUnit":"window","maxResults":1,"contextLines":0});
    let mut rows = Vec::new();
    loop {
        let d = data(&c, "ik_search", args);
        for line in d["results"][0]["lines"].as_array().unwrap() {
            rows.push(line["number"].as_u64().unwrap());
        }
        if d["nextCall"].is_null() {
            break;
        }
        args = d["nextCall"]["arguments"].clone();
    }
    assert_eq!(rows, vec![4, 5]);
}

#[test]
fn coverage_merges_duplicates_and_detects_partial_and_missing_docs() {
    let c = corpus();
    let catalog = data(&c, "ik_catalog", json!({"path":"3.3版本","manifest":true}));
    let id = catalog["manifest"]["manifestId"].clone();
    let read = data(&c, "ik_read", json!({"docId":c.doc_id(0),"limit":2}));
    let mut ranges = read["coverageDelta"].as_array().unwrap().clone();
    ranges.extend(ranges.clone());
    let mut overlap = ranges[0].clone();
    overlap["start"] = json!(2);
    overlap["end"] = json!(3);
    ranges.push(overlap);
    let d = data(&c, "ik_coverage", json!({"manifestId":id,"ranges":ranges}));
    assert_eq!(d["coveredLines"], 3);
    assert_eq!(d["partialDocuments"], 1);
    assert_eq!(d["unreadDocuments"], 6);
    assert_eq!(d["completeDocuments"], 0);
    assert_eq!(d["ledger"].as_array().unwrap().len(), 1);
    assert_eq!(d["pending"][0]["missing"][0]["start"], 4);
    let mut ranges = Vec::new();
    for i in 0..6 {
        ranges.extend(
            data(&c, "ik_read", json!({"docId":c.doc_id(i)}))["coverageDelta"]
                .as_array()
                .unwrap()
                .clone(),
        );
    }
    let d = data(&c, "ik_coverage", json!({"manifestId":id,"ranges":ranges}));
    assert_eq!(d["completeDocuments"], 6);
    assert_eq!(d["unreadDocuments"], 1);
    assert_eq!(d["coveredLines"], 30);
}

#[test]
fn batch_reads_keep_source_identity_and_continue_with_one_argument() {
    let c = corpus();
    let mut args = json!({"docIds":[c.doc_id(0),c.doc_id(1)],"limit":3,"lineNumbers":false});
    let mut count = 0;
    loop {
        let response = rpc(&c, "ik_read", args);
        assert_eq!(response["result"]["isError"], false, "{response}");
        let d = &response["result"]["structuredContent"]["data"];
        for block in d["blocks"].as_array().unwrap() {
            for line in block["lines"].as_array().unwrap() {
                assert!(line["sourceKey"].is_string());
                assert_eq!(line["complete"], true);
                count += 1;
            }
        }
        assert!(
            !response["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("L1 ")
        );
        if d["nextCall"].is_null() {
            break;
        }
        assert_eq!(d["nextCall"]["arguments"].as_object().unwrap().len(), 1);
        args = d["nextCall"]["arguments"].clone();
    }
    assert_eq!(count, 10);
    assert_eq!(
        rpc(
            &c,
            "ik_read",
            json!({"docIds":[c.doc_id(0)],"docId":c.doc_id(0)})
        )["result"]["isError"],
        true
    );
}

#[test]
fn cursor_rejects_scope_changes_tampering_and_new_snapshot() {
    let c = corpus();
    let first = data(&c, "ik_search", json!({"keyword":"天空","maxResults":1}));
    let args = first["nextCall"]["arguments"].clone();
    let mut conflict = args.clone();
    conflict["scope"] = json!("lore");
    assert_eq!(rpc(&c, "ik_search", conflict)["result"]["isError"], true);
    assert_eq!(rpc(&c, "ik_read", args.clone())["result"]["isError"], true);
    let mut changed = corpus();
    changed.revision.push('x');
    assert_eq!(
        rpc(&changed, "ik_search", args.clone())["result"]["isError"],
        true
    );
    let mut bad = args.clone();
    let token = bad["cursor"].as_str().unwrap().to_owned();
    bad["cursor"] = json!(format!("!{}", &token[1..]));
    assert_eq!(rpc(&c, "ik_search", bad)["result"]["isError"], true);
    assert_eq!(
        rpc(&corpus(), "ik_search", args)["result"]["isError"],
        false
    );
}

#[test]
fn manifest_exclusions_and_snapshot_validation_are_explicit() {
    let c = corpus();
    let cat = data(
        &c,
        "ik_catalog",
        json!({"path":"3.3版本","manifest":true,"excludeUnlinked":true}),
    );
    assert_eq!(cat["manifest"]["excludedDocuments"], 7);
    assert_eq!(cat["manifest"]["totalDocuments"], 0);
    assert_eq!(cat["manifest"]["excludedLines"], 35);
    let cat = data(&c, "ik_catalog", json!({"path":"3.3版本","manifest":true}));
    let id = cat["manifest"]["manifestId"].clone();
    let mut wrong = data(&c, "ik_read", json!({"docId":c.doc_id(0)}))["coverageDelta"].clone();
    wrong[0]["corpusRevision"] = json!("old");
    assert_eq!(
        rpc(&c, "ik_coverage", json!({"manifestId":id,"ranges":wrong}))["result"]["isError"],
        true
    );
    let mut truncated = data(&c, "ik_read", json!({"docId":c.doc_id(0)}))["coverageDelta"].clone();
    truncated[0]["complete"] = json!(false);
    assert_eq!(
        data(
            &c,
            "ik_coverage",
            json!({"manifestId":id,"ranges":truncated})
        )["coveredLines"],
        0
    );
}

#[test]
fn generated_schemas_are_self_contained_and_match_results() {
    let c = corpus();
    let defs = ik_story::mcp::tools::definitions().unwrap();
    fn refs(v: &Value, root: &Value) {
        match v {
            Value::Object(m) => {
                if let Some(r) = m.get("$ref").and_then(Value::as_str) {
                    assert!(r.starts_with("#/$defs/"), "{r}");
                    assert!(
                        !root.pointer(&r[1..]).unwrap_or(&Value::Null).is_null(),
                        "{r}"
                    );
                }
                for v in m.values() {
                    refs(v, root);
                }
            }
            Value::Array(a) => {
                for v in a {
                    refs(v, root);
                }
            }
            _ => {}
        }
    }
    for entry in defs["tools"].as_array().unwrap() {
        assert_eq!(entry["outputSchema"]["type"], "object");
        refs(&entry["outputSchema"], &entry["outputSchema"]);
    }
    let compact = rpc(
        &c,
        "ik_search",
        json!({"keyword":"天空 核心","maxResults":2}),
    );
    let full = rpc(
        &c,
        "ik_search",
        json!({"keyword":"天空 核心","maxResults":2,"detail":"full"}),
    );
    assert_eq!(
        compact["result"]["structuredContent"]["data"]["results"],
        full["result"]["structuredContent"]["data"]["results"]
    );
    assert!(
        compact["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .len()
            < full["result"]["content"][0]["text"].as_str().unwrap().len()
    );
    if let Ok(path) = std::env::var("STORY_SCHEMA_REPORT") {
        let catalog = rpc(&c, "ik_catalog", json!({"path":"3.3版本","manifest":true}));
        let read = rpc(&c, "ik_read", json!({"docId":c.doc_id(0),"limit":2}));
        let coverage = rpc(
            &c,
            "ik_coverage",
            json!({"manifestId":catalog["result"]["structuredContent"]["data"]["manifest"]["manifestId"],"ranges":read["result"]["structuredContent"]["data"]["coverageDelta"]}),
        );
        let guide = rpc(&c, "ik_guide", json!({"mode":"summary"}));
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&json!({"definitions":defs,"responses":[{"tool":"ik_search","response":compact},{"tool":"ik_search","response":full},{"tool":"ik_catalog","response":catalog},{"tool":"ik_read","response":read},{"tool":"ik_coverage","response":coverage},{"tool":"ik_guide","response":guide}]}))
                .unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn window_fallback_keeps_documents_that_have_only_one_term() {
    let mut bundle = Bundle {
        format: 1,
        manifest: None,
        story: String::new(),
        supplement: None,
        lore: Default::default(),
    };
    bundle.story = [
        ("0", 0, "甲"),
        ("0", 1, "空"),
        ("0", 2, "空"),
        ("0", 3, "乙"),
        ("1", 0, "甲"),
    ]
    .iter()
    .map(|(s, o, t)| {
        json!({"chapter":"Chapter240","family":"GalGame","scene":s,"order":o,"text":t}).to_string()
    })
    .collect::<Vec<_>>()
    .join("\n");
    let c = build_from_bundle(&bundle).unwrap();
    let d = data(
        &c,
        "ik_search",
        json!({"keyword":"甲 乙","matchUnit":"window","windowLines":2}),
    );
    assert_eq!(d["candidateCount"], 2);
    assert_eq!(d["partial"], true);
}

#[test]
fn windows_are_minimal_and_show_local_order_evidence() {
    let mut bundle = Bundle {
        format: 1,
        manifest: None,
        story: String::new(),
        supplement: None,
        lore: Default::default(),
    };
    bundle.story=["甲","乙","乙"].iter().enumerate().map(|(i,t)|json!({"chapter":"Chapter240","family":"GalGame","scene":"0","order":i,"text":t,"line_order_source":"archive_table","line_order_status":"unknown"}).to_string()).collect::<Vec<_>>().join("\n");
    let c = build_from_bundle(&bundle).unwrap();
    let d = data(
        &c,
        "ik_search",
        json!({"keyword":"甲 乙","matchUnit":"window","windowLines":3}),
    );
    assert_eq!(d["results"][0]["windows"].as_array().unwrap().len(), 1);
    assert_eq!(d["results"][0]["windows"][0]["end"], 2);
    assert_eq!(
        d["results"][0]["windows"][0]["orderSource"],
        "archive_table"
    );
}

#[test]
fn within_doc_context_does_not_count_as_repeated_match_on_next_page() {
    let c = corpus();
    let first = data(
        &c,
        "ik_search",
        json!({"keyword":"重复","docId":c.doc_id(0),"maxResults":1,"contextLines":1}),
    );
    assert_eq!(first["results"][0]["lines"][2]["isContext"], true);
    assert_eq!(first["results"][0]["matches"][0]["lines"], json!([4]));
    let next = data(&c, "ik_search", first["nextCall"]["arguments"].clone());
    assert_eq!(next["results"][0]["lines"][0]["isContext"], true);
    assert_eq!(next["results"][0]["matches"][0]["lines"], json!([5]));
}

#[test]
fn catalog_and_coverage_page_all_members_without_duplicates() {
    let c = corpus();
    let mut args = json!({"path":"3.3版本","maxResults":2,"manifest":true});
    let mut ids = std::collections::HashSet::new();
    let mut manifest = Value::Null;
    loop {
        let d = data(&c, "ik_catalog", args);
        if manifest.is_null() {
            manifest = d["manifest"]["manifestId"].clone();
        } else {
            assert_eq!(manifest, d["manifest"]["manifestId"]);
        }
        for doc in d["documents"].as_array().unwrap() {
            assert!(ids.insert(doc["docId"].as_str().unwrap().to_owned()));
        }
        if d["nextCall"].is_null() {
            break;
        }
        args = d["nextCall"]["arguments"].clone();
    }
    assert_eq!(ids.len(), 7);
    ids.clear();
    let mut args = json!({"manifestId":manifest,"maxResults":2});
    loop {
        let d = data(&c, "ik_coverage", args);
        assert_eq!(d["coveredLines"], 0);
        for doc in d["pending"].as_array().unwrap() {
            assert!(ids.insert(doc["document"]["docId"].as_str().unwrap().to_owned()));
            assert!(doc["missing"][0].get("complete").is_none());
        }
        if d["nextCall"].is_null() {
            break;
        }
        args = d["nextCall"]["arguments"].clone();
    }
    assert_eq!(ids.len(), 7);
}

#[test]
fn long_search_excerpts_are_truncated_and_read_is_complete() {
    let mut b = Bundle {
        format: 1,
        manifest: None,
        story: String::new(),
        supplement: None,
        lore: Default::default(),
    };
    b.story=json!({"chapter":"Chapter240","family":"GalGame","scene":"0","order":0,"text":format!("天空{}","甲".repeat(1000))}).to_string();
    let c = build_from_bundle(&b).unwrap();
    let search = data(&c, "ik_search", json!({"keyword":"天空"}));
    assert_eq!(search["results"][0]["lines"][0]["complete"], false);
    assert!(search.get("coverageDelta").is_none());
    let read = data(&c, "ik_read", json!({"docId":c.doc_id(0),"maxChars":500}));
    assert_eq!(read["blocks"][0]["lines"][0]["complete"], true);
    assert_eq!(
        read["blocks"][0]["lines"][0]["text"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        1002
    );
    assert_eq!(read["coverageDelta"][0]["end"], 1);
}

#[test]
fn task_presets_allow_explicit_overrides_and_errors_are_actionable() {
    let c = corpus();
    let d = data(
        &c,
        "ik_search",
        json!({"keyword":"天空 核心 秘密","path":"3.3版本","purpose":"verify"}),
    );
    assert_eq!(d["candidateCount"], 0);
    assert_eq!(d["resolvedRange"]["matchIn"], "body");
    let retry = data(&c, "ik_search", d["suggestions"][0]["arguments"].clone());
    assert_eq!(retry["partial"], true);
    assert_eq!(retry["resolvedRange"]["groupId"], "ch:Chapter240");
    let d = data(
        &c,
        "ik_search",
        json!({"keyword":"天空 核心 秘密","purpose":"verify","allowPartial":true,"matchIn":"all"}),
    );
    assert_eq!(d["partial"], true);
    assert_eq!(d["resolvedRange"]["matchIn"], "all");
    assert!(
        ik_story::mcp::tools::call(&c, "ik_search", Some(json!([])))
            .ok()
            .unwrap()
            .is_err()
    );
    assert_eq!(
        rpc(&c, "ik_search", json!({"cursor":"x".repeat(16385)}))["result"]["isError"],
        true
    );
    let cat = data(&c, "ik_catalog", json!({"manifest":true}));
    let invalid =
        json!({"corpusRevision":c.revision,"docId":c.doc_id(0),"start":0,"end":6,"complete":true});
    assert_eq!(
        rpc(
            &c,
            "ik_coverage",
            json!({"manifestId":cat["manifest"]["manifestId"],"ranges":[invalid]})
        )["result"]["isError"],
        true
    );
}

#[tokio::test]
async fn http_cursor_and_coverage_contracts_use_the_same_snapshot() {
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let c = corpus();
    let config = ik_kernel::config::Story {
        public: true,
        ..Default::default()
    };
    let state = ik_story::StoryState::with_corpus(c, &config).unwrap();
    let (router, _) = ik_story::mcp_router().split_for_parts();
    let app = router.with_state(state);
    async fn call(app: axum::Router, tool: &str, args: Value) -> Value {
        let request = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":tool,"arguments":args}});
        let response = app
            .oneshot(
                Request::post("/v1/story/mcp")
                    .header("content-type", "application/json")
                    .header("accept", "application/json, text/event-stream")
                    .body(Body::from(request.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
    }
    let catalog = call(
        app.clone(),
        "ik_catalog",
        json!({"path":"3.3版本","manifest":true}),
    )
    .await;
    let first = call(
        app.clone(),
        "ik_read",
        json!({"docId":"3.3版本","limit":"2","lineNumbers":"false"}),
    )
    .await;
    let second = call(
        app.clone(),
        "ik_read",
        first["result"]["structuredContent"]["data"]["nextCall"]["arguments"].clone(),
    )
    .await;
    assert_eq!(
        second["result"]["structuredContent"]["data"]["coverageDelta"][0]["start"],
        3
    );
    let mut ranges = first["result"]["structuredContent"]["data"]["coverageDelta"]
        .as_array()
        .unwrap()
        .clone();
    ranges.extend(
        second["result"]["structuredContent"]["data"]["coverageDelta"]
            .as_array()
            .unwrap()
            .clone(),
    );
    let coverage=call(app,"ik_coverage",json!({"manifestId":catalog["result"]["structuredContent"]["data"]["manifest"]["manifestId"],"ranges":ranges})).await;
    assert_eq!(
        coverage["result"]["structuredContent"]["data"]["coveredLines"],
        4
    );
    assert_eq!(
        coverage["result"]["structuredContent"]["corpusRevision"],
        catalog["result"]["structuredContent"]["corpusRevision"]
    );
}

#[test]
fn window_body_matches_do_not_become_attribution_matches() {
    let b=Bundle {format:1,manifest:None,story:json!({"chapter":"Chapter240","family":"GalGame","scene":"0","order":0,"text":"天空","speaker":"角色甲","line_type":"dialogue","speaker_status":"inferred"}).to_string(),supplement:None,lore:Default::default()};
    let c = build_from_bundle(&b).unwrap();
    for args in [
        json!({"keyword":"天空","matchUnit":"window"}),
        json!({"keyword":"天空","matchUnit":"window","matchIn":"body"}),
    ] {
        assert_eq!(
            data(&c, "ik_search", args)["results"][0]["matches"][0]["fields"],
            json!(["body"])
        );
    }
}

#[test]
fn fully_shown_window_does_not_offer_an_empty_more_matches_call() {
    let b = Bundle {
        format: 1,
        manifest: None,
        story: ["甲", "乙", "丙", "丁"]
            .iter()
            .enumerate()
            .map(|(i, t)| {
                json!({"chapter":"Chapter240","family":"GalGame","scene":"0","order":i,"text":t})
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n"),
        supplement: None,
        lore: Default::default(),
    };
    let c = build_from_bundle(&b).unwrap();
    let d = data(
        &c,
        "ik_search",
        json!({"keyword":"甲 乙 丙 丁","matchUnit":"window","hitsPerDoc":1}),
    );
    assert_eq!(d["results"][0]["windows"].as_array().unwrap().len(), 1);
    assert_eq!(d["results"][0]["lines"].as_array().unwrap().len(), 4);
    assert!(d["results"][0]["nextMatches"].is_null());
}
