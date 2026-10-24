//! extra.jsonl (activities, side quests, agent stories) becomes its own content kind.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use ik_story::corpus::{Bundle, Corpus, DocKind, GroupKind, build_from_bundle};
use ik_story::mcp::Server;
use serde_json::{Value, json};

fn rows(rows: &[Value]) -> String {
    rows.iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

fn corpus(extra: Option<String>) -> Corpus {
    let story = rows(&[
        json!({"chapter":"Chapter110","chapter_label":"第二季 第一章","chapter_order":180,
        "family":"GalGame","scene":"1","order":1,"text":"主线台词","speaker":"哲"}),
    ]);
    build_from_bundle(&Bundle {
        format: 1,
        manifest: Some(json!({"source_version":"3.3.0"})),
        story,
        supplement: None,
        extra,
        lore: Default::default(),
    })
    .unwrap()
}

fn extra() -> String {
    rows(&[
        json!({"domain":"activity","chapter":"activity.SayItWithFlowers","chapter_label":"活动剧情 · SayItWithFlowers",
            "chapter_order":1_000_000,"family":"GalGame","scene":"10161201","order":1,
            "text":"店里最受欢迎的是花束。","speaker":"花店店主","line_type":"dialogue",
            "source_key":"Activity_GalGame_SayItWithFlowers_10161201_006"}),
        json!({"domain":"activity","chapter":"activity.SayItWithFlowers","chapter_label":"活动剧情 · SayItWithFlowers",
            "chapter_order":1_000_000,"family":"GalGame","scene":"10161201","order":2,
            "text":"您请说吧","line_type":"option"}),
        json!({"domain":"side","chapter":"side.ZenkovS1","chapter_label":"支线委托 · ZenkovS1",
            "chapter_order":2_000_000,"family":"Chat","scene":"226023006","order":3,
            "text":"照小姐遗失了一张唱片《苦情慢歌周年版》。","line_type":"unknown"}),
    ])
}

fn call(c: &Corpus, tool: &str, args: Value) -> Value {
    let request = json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
        "params":{"name":tool,"arguments":args}});
    let response: Value = serde_json::from_slice(
        &Server::new()
            .unwrap()
            .handle(c, &serde_json::to_vec(&request).unwrap())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(response["result"]["isError"], false, "{response}");
    response["result"].clone()
}

fn text(result: &Value) -> &str {
    result["content"][0]["text"].as_str().unwrap()
}

#[test]
fn extra_rows_become_event_groups_after_chapters() {
    let c = corpus(Some(extra()));
    let ids: Vec<_> = c.groups.iter().map(|g| (g.id.as_str(), g.kind)).collect();
    assert_eq!(
        ids,
        [
            ("ch:Chapter110", GroupKind::Chapter),
            ("ev:activity.SayItWithFlowers", GroupKind::Event),
            ("ev:side.ZenkovS1", GroupKind::Event),
        ]
    );
    let events: Vec<_> = c.docs.iter().filter(|d| d.kind == DocKind::Event).collect();
    assert_eq!(events.len(), 2);
    assert!(
        events[0]
            .path
            .starts_with("活动与支线 › 活动剧情 · SayItWithFlowers")
    );
    // Only mainline lines count toward the reported speaker coverage.
    assert_eq!(c.meta.speaker_coverage, 1.0);
    assert_eq!(
        c.find_group("ev:side.zenkovs1"),
        c.find_group("ev:side.ZenkovS1")
    );
}

#[test]
fn bundles_without_extra_still_load() {
    let c = corpus(None);
    assert!(c.docs.iter().all(|d| d.kind != DocKind::Event));
    let bundle: Bundle =
        serde_json::from_value(json!({"format":1,"story":"{\"chapter\":\"Chapter00\",\"family\":\"GalGame\",\"scene\":\"1\",\"text\":\"旧包\"}"}))
            .unwrap();
    assert!(bundle.extra.is_none());
}

#[test]
fn root_catalog_collapses_events_and_scope_lists_them() {
    let c = corpus(Some(extra()));
    let root = call(&c, "ik_catalog", json!({}));
    let body = text(&root);
    assert!(body.contains("活动与支线"), "{body}");
    assert!(body.contains("2 组 · 2 篇 · 3 行"), "{body}");
    assert!(!body.contains("ev:side.ZenkovS1"), "{body}");
    let full_root = call(&c, "ik_catalog", json!({"detail":"full"}));
    assert!(
        text(&full_root).contains("2 组 · 2 篇 · 3 行"),
        "{}",
        text(&full_root)
    );

    let scoped = call(&c, "ik_catalog", json!({"scope":"event"}));
    let groups = scoped["structuredContent"]["data"]["groups"]
        .as_array()
        .or_else(|| scoped["structuredContent"]["groups"].as_array())
        .unwrap_or_else(|| panic!("{scoped}"));
    let ids: Vec<_> = groups
        .iter()
        .map(|g| g["groupId"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["ev:activity.SayItWithFlowers", "ev:side.ZenkovS1"]);

    let listing = call(&c, "ik_catalog", json!({"path":"ev:side.ZenkovS1"}));
    assert!(
        text(&listing).contains("[ik_catalog] 1 篇 · 1 行"),
        "{}",
        text(&listing)
    );
    let full = call(
        &c,
        "ik_catalog",
        json!({"path":"ev:side.ZenkovS1","detail":"full"}),
    );
    assert!(text(&full).contains("目录顺序"), "{}", text(&full));
}

#[test]
fn search_and_read_reach_event_text() {
    let c = corpus(Some(extra()));
    let hit = call(
        &c,
        "ik_search",
        json!({"keyword":"苦情慢歌","scope":"支线"}),
    );
    assert!(text(&hit).contains("命中 1 篇"), "{}", text(&hit));
    let story_only = call(
        &c,
        "ik_search",
        json!({"keyword":"苦情慢歌","scope":"story"}),
    );
    assert!(
        text(&story_only).contains("命中 0 篇"),
        "{}",
        text(&story_only)
    );

    let read = call(
        &c,
        "ik_read",
        json!({"docId":"ev:activity.SayItWithFlowers"}),
    );
    let body = text(&read);
    assert!(body.contains("花店店主"), "{body}");
    assert!(body.contains("您请说吧"), "{body}");
}
