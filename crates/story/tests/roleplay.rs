//! ik_character(purpose="roleplay")：扮演参考资料的抽样与边界。
//! 合成语料，不依赖游戏原文。

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use ik_story::corpus::{Bundle, Corpus, build_from_bundle};
use ik_story::mcp::tools;

const HERO: &str = "测试主角";

/// 4 章 × 5 场。每场：铃→主角（长句）、哲→主角（短句「嗯。」）、主角推断署名、选项、主角口头禅（全库重复）。
/// 第 4 章第 0 场额外有一组「哲→主角」的可抽样往来；另有相关名称「测试主角B」。
fn corpus() -> Corpus {
    let mut rows = Vec::new();
    let mut order = 0;
    let mut push = |c: usize,
                    s: usize,
                    speaker: Option<&str>,
                    kind: &str,
                    status: &str,
                    text: String| {
        order += 1;
        let mut v = json!({
            "chapter":format!("Chapter0{c}"),"chapter_label":format!("测试第{c}章"),"chapter_order":c,
            "family":"GalGame","scene":format!("{c}{s}"),"scene_title":format!("第{c}章场景{s}"),
            "order":order,"text":text,"line_type":kind,"speaker_status":status,
            "source_key":format!("key_{order}"),"line_id":format!("line_{order}")
        });
        if let Some(sp) = speaker {
            v["speaker"] = json!(sp);
        }
        rows.push(v.to_string());
    };
    for c in 1..=4 {
        for s in 0..5 {
            push(
                c,
                s,
                Some("铃"),
                "dialogue",
                "resolved",
                format!("第{c}章第{s}场，铃问：今天的委托要紧吗？"),
            );
            push(
                c,
                s,
                Some(HERO),
                "dialogue",
                "resolved",
                format!("第{c}章第{s}场，主角回答铃：放心，这次我来带队。"),
            );
            push(
                c,
                s,
                Some("哲"),
                "dialogue",
                "resolved",
                format!("第{c}章第{s}场，哲提醒大家注意安全。"),
            );
            push(c, s, Some(HERO), "dialogue", "resolved", "嗯。".into());
            push(
                c,
                s,
                Some(HERO),
                "dialogue",
                "inferred",
                format!("推断署名的台词{c}{s}，不应被抽中。"),
            );
            push(c, s, None, "option", "resolved", format!("选项内容{c}{s}"));
            push(
                c,
                s,
                Some(HERO),
                "dialogue",
                "resolved",
                "口头禅：交给我吧！".into(),
            );
            if c == 4 && s == 0 {
                push(
                    c,
                    s,
                    Some("哲"),
                    "dialogue",
                    "resolved",
                    "第四章，哲又开口。".into(),
                );
                push(
                    c,
                    s,
                    Some(HERO),
                    "dialogue",
                    "resolved",
                    "知道了，哲。".into(),
                );
                push(
                    c,
                    s,
                    Some("测试主角B"),
                    "dialogue",
                    "resolved",
                    "我是另一个同名前缀的角色。".into(),
                );
            }
        }
    }
    let mut lore = BTreeMap::new();
    let long: String = (0..80)
        .map(|i| format!("档案第{i}行：测试主角的设定说明文字。\n"))
        .collect();
    lore.insert(
        "agents/01_测试阵营.md".to_owned(),
        format!("# 测试阵营\n\n### 测试主角（测试全名）\n\n{long}\n### 档案角色\n\n只在设定里出现的角色。\n"),
    );
    build_from_bundle(&Bundle {
        format: 1,
        manifest: Some(json!({"source_version":"3.3.0","accuracy_first":false})),
        story: rows.join("\n"),
        supplement: None,
        lore,
    })
    .unwrap()
}

fn call(c: &Corpus, args: Value) -> Result<(String, Value), String> {
    match tools::call_output(c, "ik_character", Some(args)) {
        Ok(Ok(out)) => {
            let response = serde_json::to_value(&out.response).unwrap();
            Ok((out.text, response["data"].clone()))
        }
        Ok(Err(e)) => Err(e.0),
        Err(_) => panic!("unknown tool"),
    }
}

fn texts(data: &Value) -> Vec<String> {
    data["samples"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["line"]["text"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn samples_spread_across_chapters_in_reading_order_and_are_deterministic() {
    let c = corpus();
    let args = json!({"name":HERO,"purpose":"roleplay","maxResults":8});
    let (text, data) = call(&c, args.clone()).unwrap();
    assert_eq!(data["purpose"], "roleplay");
    let samples = data["samples"].as_array().unwrap();
    assert_eq!(samples.len(), 8, "{text}");
    let chapters: BTreeSet<&str> = samples
        .iter()
        .map(|s| s["groupId"].as_str().unwrap())
        .collect();
    assert_eq!(chapters.len(), 4, "样本应覆盖全部章节：{chapters:?}");
    // 按语料顺序排列。
    let positions: Vec<(usize, u64)> = samples
        .iter()
        .map(|s| {
            (
                c.find_doc(s["docId"].as_str().unwrap()).unwrap(),
                s["line"]["number"].as_u64().unwrap(),
            )
        })
        .collect();
    let mut sorted = positions.clone();
    sorted.sort();
    assert_eq!(positions, sorted);
    // 同一快照同一参数，结果完全一致。
    assert_eq!(call(&c, args).unwrap().1, data);
    // 每条样本都有可直接调用的原文入口，覆盖样本所在行。
    for s in samples {
        let read = &s["read"];
        assert_eq!(read["tool"], "ik_read");
        assert_eq!(read["arguments"]["docId"], s["docId"]);
        let offset = read["arguments"]["offset"].as_u64().unwrap();
        let line = s["line"]["number"].as_u64().unwrap();
        assert!(offset <= line && line < offset + read["arguments"]["limit"].as_u64().unwrap());
    }
}

#[test]
fn only_reliable_distinct_dialogue_of_this_character_is_sampled() {
    let c = corpus();
    let (text, data) = call(
        &c,
        json!({"name":HERO,"purpose":"roleplay","maxResults":60}),
    )
    .unwrap();
    let all = texts(&data);
    assert!(all.iter().all(|t| !t.contains("推断署名")), "{text}");
    assert!(all.iter().all(|t| !t.contains("选项内容")), "{text}");
    assert!(all.iter().all(|t| t != "嗯。"), "{text}");
    assert_eq!(
        all.iter().filter(|t| t.contains("交给我吧")).count(),
        1,
        "{text}"
    );
    for s in data["samples"].as_array().unwrap() {
        assert_eq!(s["line"]["speaker"], HERO);
        assert_eq!(s["line"]["kind"], "dialogue");
    }
    // 20 场 × 3 句署名对白 + 第 4 章多出的 1 句；推断署名不计。
    assert_eq!(data["spokenLines"], 61);
    assert_eq!(data["scenes"], 20);
    // 20 句长回答 + 1 句口头禅 + 1 句「知道了，哲。」
    assert_eq!(data["candidateLines"], 22);
    assert_eq!(all.len(), 22);
    // 名字包含主角名的另一个说话人只列为相关名称，台词不混进来。
    assert_eq!(data["relatedNames"], json!(["测试主角B"]));
    assert!(text.contains("相关名称（未合并）：测试主角B"), "{text}");
}

#[test]
fn frequent_partners_are_counted_and_each_gets_an_exchange_sample() {
    let c = corpus();
    let (text, data) = call(&c, json!({"name":HERO,"purpose":"roleplay","maxResults":8})).unwrap();
    let partners: BTreeMap<&str, (u64, u64)> = data["partners"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["name"].as_str().unwrap(),
                (
                    p["exchanges"].as_u64().unwrap(),
                    p["scenes"].as_u64().unwrap(),
                ),
            )
        })
        .collect();
    // 往来按相邻的两句署名对白计：铃→主角每场 1 次；哲夹在主角两句之间，每场 2 次，
    // 第 4 章多出「主角→哲→主角」2 次。测试主角B 是相关名称，不算对话对象。
    assert_eq!(partners.get("铃"), Some(&(20, 20)), "{text}");
    assert_eq!(partners.get("哲"), Some(&(42, 20)), "{text}");
    assert!(!partners.contains_key("测试主角B"));
    assert!(!partners.contains_key(HERO));
    // 「知道了，哲。」比其余样本都短，只有按对话对象保底才会被选中。
    let contexts: BTreeSet<&str> = data["samples"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["context"]["speaker"].as_str())
        .collect();
    assert!(contexts.contains("铃") && contexts.contains("哲"), "{text}");
    assert!(texts(&data).contains(&"知道了，哲。".to_owned()), "{text}");
    // 前一句同时出现在文本里，样本的说话对象一眼可见。
    assert!(text.contains("哲：第四章，哲又开口。\nL"), "{text}");
}

#[test]
fn until_limits_lines_to_the_chapter_but_keeps_the_profile() {
    let c = corpus();
    let (text, data) = call(
        &c,
        json!({"name":HERO,"purpose":"roleplay","until":"ch:Chapter02","maxResults":60}),
    )
    .unwrap();
    assert_eq!(data["until"]["groupId"], "ch:Chapter02");
    assert_eq!(data["spokenLines"], 30);
    for s in data["samples"].as_array().unwrap() {
        let g = s["groupId"].as_str().unwrap();
        assert!(g == "ch:Chapter01" || g == "ch:Chapter02", "{g}");
    }
    assert!(!data["profiles"].as_array().unwrap().is_empty());
    assert!(text.contains("截止 ch:Chapter02 测试第2章（含）"), "{text}");
    assert!(text.contains("设定档案不随截止过滤"), "{text}");
    // 章节名同样可用。
    let (_, by_label) = call(
        &c,
        json!({"name":HERO,"purpose":"roleplay","until":"测试第2章"}),
    )
    .unwrap();
    assert_eq!(by_label["until"]["groupId"], "ch:Chapter02");

    let e = call(
        &c,
        json!({"name":HERO,"purpose":"roleplay","until":"lore:agents"}),
    )
    .unwrap_err();
    assert!(e.contains("设定分类"), "{e}");
    assert!(
        call(
            &c,
            json!({"name":HERO,"purpose":"roleplay","until":"不存在的章"})
        )
        .is_err()
    );
    let e = call(&c, json!({"name":HERO,"until":"ch:Chapter02"})).unwrap_err();
    assert!(e.contains("purpose=roleplay"), "{e}");
}

#[test]
fn long_profiles_are_excerpted_with_a_continuation_call() {
    let c = corpus();
    let (text, data) = call(&c, json!({"name":HERO,"purpose":"roleplay"})).unwrap();
    let p = &data["profiles"][0];
    assert_eq!(p["title"], "测试主角（测试全名）");
    assert_eq!(p["truncated"], true);
    assert!(p["excerpt"].as_str().unwrap().chars().count() <= 1500);
    let taken = p["excerptLines"].as_u64().unwrap();
    assert!(taken < p["lineCount"].as_u64().unwrap());
    assert_eq!(p["nextRead"]["arguments"]["offset"], taken + 1);
    assert!(text.contains("续读：ik_read(docId="), "{text}");

    // 续读入口接得上：从摘录的下一行开始。
    let next = &p["nextRead"]["arguments"];
    let Ok(Ok(read)) = tools::call_output(&c, "ik_read", Some(next.clone())) else {
        panic!("ik_read failed");
    };
    let excerpt_last = p["excerpt"].as_str().unwrap().lines().last().unwrap();
    assert!(!read.text.contains(excerpt_last), "{}", read.text);
}

#[test]
fn names_must_identify_one_character() {
    let c = corpus();
    // 「主角」只是包含匹配，同时命中两个角色：不混在一起。
    let e = call(&c, json!({"name":"主角","purpose":"roleplay"})).unwrap_err();
    assert!(e.contains("测试主角") && e.contains("测试主角B"), "{e}");

    // 只有设定、没有台词的角色：给档案，不给台词检索入口。
    let (text, data) = call(&c, json!({"name":"档案角色","purpose":"roleplay"})).unwrap();
    assert!(text.contains("不是已知说话人"), "{text}");
    assert_eq!(data["samples"], json!([]));
    assert_eq!(data["nextCalls"], json!([]));
    assert_eq!(data["profiles"][0]["title"], "档案角色");

    assert!(call(&c, json!({"name":"查无此人","purpose":"roleplay"})).is_err());
    let e = call(
        &c,
        json!({"name":HERO,"purpose":"roleplay","includeEvidence":true}),
    )
    .unwrap_err();
    assert!(e.contains("includeEvidence=true"), "{e}");
    let e = call(&c, json!({"name":HERO,"purpose":"bio"})).unwrap_err();
    assert!(e.contains("overview / roleplay"), "{e}");
}

#[test]
fn overview_is_unchanged_and_output_schema_covers_both_shapes() {
    let c = corpus();
    let (_, overview) = call(&c, json!({"name":HERO})).unwrap();
    assert!(overview.get("purpose").is_none());
    assert!(overview["chapters"].is_array());

    let defs = tools::definitions().unwrap();
    let character = defs["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "ik_character")
        .unwrap();
    let schema = character["outputSchema"].to_string();
    assert!(schema.contains("#/$defs/PersonaResult"), "{schema}");
    assert!(schema.contains("#/$defs/CharacterResult"), "{schema}");
    let input = &character["inputSchema"]["properties"];
    assert_eq!(input["purpose"]["enum"], json!(["overview", "roleplay"]));
    assert!(input["until"].is_object());
}
