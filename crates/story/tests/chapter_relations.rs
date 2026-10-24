//! Chapter hints retain raw identities and separate equal labels from reviewed mappings.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use ik_story::corpus::{Bundle, Corpus, build_from_bundle};
use serde_json::{Value, json};

fn corpus() -> Corpus {
    let main = [
        ("Chapter02", "第一季 第二章"),
        ("Chapter025", "第一季 第二章间章"),
        ("Chapter04", "第一季 第四章"),
        ("Chapter040", "第一季 第四章"),
        ("Chapter05", "第一季 第五章"),
        ("Chapter050", "第一季 第五章"),
    ];
    let supplementary = [
        ("Chapter028", "第一季 第二章"),
        ("Chapter052", "第一季 第五章"),
        ("Chapter055", "第一季 第五章间章"),
        ("Chapter058", "第一季 第五章"),
    ];
    let rows = |groups: &[(&str, &str)], family: &str| {
        groups.iter().enumerate().map(|(order,(chapter,label))| {
            json!({"chapter":chapter,"chapter_label":label,"family":family,"scene":"1","order":order,"text":"测试原文"}).to_string()
        }).collect::<Vec<_>>().join("\n")
    };
    build_from_bundle(&Bundle {
        format: 1,
        manifest: Some(json!({"source_version":"3.3.0"})),
        story: rows(&main, "GalGame"),
        supplement: Some(rows(&supplementary, "CS")),
        extra: None,
        lore: Default::default(),
    })
    .unwrap()
}

fn relations(c: &Corpus) -> Vec<Value> {
    serde_json::to_value(ik_story::engine::chapter_relations::relations(c))
        .unwrap()
        .as_array()
        .unwrap()
        .clone()
}

#[test]
fn hints_keep_main_interlude_and_supplement_relations_separate() {
    let c = corpus();
    let original_ids = c.groups.iter().map(|g| g.id.clone()).collect::<Vec<_>>();
    let original_revision = c.revision.clone();
    let result = relations(&c);
    assert!(
        result.iter().any(|r| r["relation"] == "sharedLabel"
            && r["memberGroupIds"] == json!(["ch:Chapter04", "ch:Chapter040"])
            && r["status"] == "labelOnly"),
        "{result:?}"
    );
    assert!(result.iter().any(|r| r["relation"] == "supplementOf"
        && r["memberGroupIds"] == json!(["ch:Chapter052", "ch:Chapter058"])
        && r["relatedGroupIds"] == json!(["ch:Chapter05", "ch:Chapter050"])));
    assert!(result.iter().any(|r| r["relation"] == "interludeOf"
        && r["memberGroupIds"] == json!(["ch:Chapter025"])
        && r["relatedGroupIds"] == json!(["ch:Chapter02"])));
    assert!(result.iter().any(|r| r["relation"] == "interludeOf"
        && r["memberGroupIds"] == json!(["ch:Chapter055"])
        && r["relatedGroupIds"] == json!(["ch:Chapter05", "ch:Chapter050"])));
    assert!(!result.iter().any(|r| {
        r["relation"] == "sharedLabel"
            && r["memberGroupIds"]
                .as_array()
                .unwrap()
                .iter()
                .any(|id| id == "ch:Chapter025")
    }));
    assert_eq!(
        original_ids,
        c.groups.iter().map(|g| g.id.clone()).collect::<Vec<_>>()
    );
    assert_eq!(original_revision, c.revision);
    for r in result.iter().filter(|r| r["relation"] != "sharedLabel") {
        assert_eq!(r["status"], "reviewedExtractorMapping");
        assert_eq!(r["source"]["kind"], "extractorMapping");
        assert!(r["source"]["commit"].as_str().unwrap().len() >= 7);
        assert_eq!(r["source"]["path"], "zzz_story/extract.py");
        assert!(r["source"]["startLine"].as_u64().unwrap() > 0);
    }
    assert_eq!(relations(&c), result);
}

#[test]
fn absent_or_unrelated_groups_do_not_create_navigation_hints() {
    let mut c = corpus();
    c.groups.retain(|g| g.id == "ch:Chapter04");
    assert!(relations(&c).is_empty());
}

#[test]
fn changed_labels_do_not_reuse_first_season_mapping() {
    let mut c = corpus();
    for group in &mut c.groups {
        if group.id == "ch:Chapter052" || group.id == "ch:Chapter058" {
            group.label = "新版本独立章节".into();
        }
    }
    let result = relations(&c);
    assert!(
        !result.iter().any(|r| {
            r["relation"] == "supplementOf"
                && r["memberGroupIds"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|id| id == "ch:Chapter052" || id == "ch:Chapter058")
        }),
        "{result:?}"
    );
    assert!(result.iter().any(|r| r["relation"] == "sharedLabel"
        && r["memberGroupIds"] == json!(["ch:Chapter052", "ch:Chapter058"])));

    for group in &mut c.groups {
        if group.id == "ch:Chapter02" {
            group.label = "其他产品章节".into();
        }
    }
    assert!(
        !relations(&c)
            .iter()
            .any(|r| r["relation"] == "interludeOf"
                && r["memberGroupIds"] == json!(["ch:Chapter025"]))
    );
}
