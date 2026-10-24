//! ik_character(purpose="roleplay")：档案小节、档案对话抽取与主线样本的边界。
//! 合成语料，不依赖游戏原文；档案格式与提取器输出的代理人档案一致。

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use ik_story::corpus::{Bundle, Corpus, build_from_bundle};
use ik_story::mcp::tools;

const HERO: &str = "测试主角";

/// 主角的代理人档案：身份/性格小节、一个只给入口的小节，以及三类对话。
/// - 信赖邀约 2 幕：「长邀约」40 行、「短邀约」5 行；
/// - 街区日常 10 幕：偶数幕 6 行，奇数幕 2 行；
/// - 随行气泡 15 组，每组 2 行。
fn hero_profile() -> String {
    let mut md = String::from(
        "### 测试主角（测试 全名）\n\n| 属性 | 档案记录 |\n| :--- | :--- |\n| **所属阵营** | 测试阵营 |\n\
         #### 空洞调查协会（HIA）执照与以太适性档案\n> 执照信息。\n\
         #### 深度身世与机密档案\n**档案记录 01**：\n测试主角性格开朗，口头禅是「交给我吧」。\n\
         #### 录像店私密评语（绳匠视角）\n- 哲：测试评语。\n\
         #### 意象影画·心象世界（共 2 幕）\n##### 第 1 幕：甲\n心象一。\n##### 第 2 幕：乙\n心象二。\n\
         #### 专属信赖邀约（GalGame事件，共 2 幕）\n##### 「长邀约」\n",
    );
    for i in 0..40 {
        md.push_str(&format!("- 【测试主角】邀约第{i}句。 (`Key_T1_{i}`)\n"));
    }
    md.push_str("##### 「短邀约」\n");
    for i in 0..5 {
        md.push_str(&format!("- 【测试主角】短邀约{i}。 (`Key_T2_{i}`)\n"));
    }
    md.push_str("#### 街区偶遇与日常对话（共 10 幕）\n");
    for k in 0..10 {
        md.push_str(&format!("##### 街区日常 {k}\n"));
        let n = if k % 2 == 0 { 6 } else { 2 };
        for j in 0..n {
            if j % 2 == 0 {
                md.push_str(&format!(
                    "- 【测试主角】日常{k}第{j}句，店长。 (`Key_D{k}_{j}`)\n"
                ));
            } else {
                md.push_str(&format!("- 玩家选项{k}{j} (`Key_D{k}_{j}`)\n"));
            }
        }
    }
    md.push_str("#### 随行气泡与特别对话（共 15 组）\n");
    for g in 0..15 {
        md.push_str(&format!(
            "##### 街区气泡 {g}\n- 气泡{g}第一句。 (`Key_B{g}_1`)\n- 气泡{g}第二句。 (`Key_B{g}_2`)\n"
        ));
    }
    md
}

/// 主线：4 章 × 5 场。每场：铃→主角（长句）、哲→主角（短句「嗯。」）、主角推断署名、选项、主角口头禅（全库重复）。
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
    lore.insert(
        "agents/01_测试阵营.md".to_owned(),
        format!(
            "# 测试阵营\n\n{}\n### 测试主角B（测试主角B）\n\n另一个角色。\n\
             ### 档案角色（档案角色）\n\n#### 深度身世与机密档案\n只在设定里出现的角色。\n\
             ### 仅邀约（仅邀约）\n\n#### 专属信赖邀约（GalGame事件，共 3 幕）\n\
             ##### 「一」\n- 【仅邀约】一。\n##### 「二」\n- 【仅邀约】二。\n##### 「三」\n- 【仅邀约】三。\n",
            hero_profile()
        ),
    );
    build_from_bundle(&Bundle {
        format: 1,
        manifest: Some(json!({"source_version":"3.3.0","accuracy_first":false})),
        story: rows.join("\n"),
        supplement: None,
        extra: None,
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

fn read(c: &Corpus, args: &Value) -> String {
    let Ok(Ok(out)) = tools::call_output(c, "ik_read", Some(args.clone())) else {
        panic!("ik_read failed: {args}");
    };
    out.text
}

fn sample_texts(data: &Value) -> Vec<String> {
    data["storySamples"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["line"]["text"].as_str().unwrap().to_owned())
        .collect()
}

fn scene_headings(data: &Value) -> Vec<String> {
    data["dialogueScenes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["heading"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn profile_sections_are_quoted_and_other_sections_are_linked() {
    let c = corpus();
    let (text, data) = call(&c, json!({"name":HERO,"purpose":"roleplay"})).unwrap();
    let profile = &data["profile"];
    assert_eq!(profile["title"], "测试主角（测试 全名）");
    let sections = profile["sections"].as_array().unwrap();
    let kinds: Vec<(&str, &str)> = sections
        .iter()
        .map(|s| (s["heading"].as_str().unwrap(), s["kind"].as_str().unwrap()))
        .collect();
    assert_eq!(
        kinds,
        [
            ("（开头）", "profile"),
            ("空洞调查协会（HIA）执照与以太适性档案", "profile"),
            ("深度身世与机密档案", "profile"),
            ("录像店私密评语（绳匠视角）", "profile"),
            ("意象影画·心象世界（共 2 幕）", "other"),
            ("专属信赖邀约（GalGame事件，共 2 幕）", "dialogue"),
            ("街区偶遇与日常对话（共 10 幕）", "dialogue"),
            ("随行气泡与特别对话（共 15 组）", "dialogue"),
        ]
    );
    // 身份与性格小节原样给出。
    assert!(
        text.contains("### 深度身世与机密档案\n**档案记录 01**：\n测试主角性格开朗"),
        "{text}"
    );
    assert!(sections[2]["text"].as_str().unwrap().contains("口头禅"));
    // 其余小节只给入口，入口从小节标题行开始。
    let other = &sections[4];
    assert!(other.get("text").is_none());
    assert!(
        text.contains("其他小节：意象影画·心象世界（共 2 幕） → ik_read("),
        "{text}"
    );
    let body = read(&c, &other["read"]["arguments"]);
    assert!(body.contains("#### 意象影画·心象世界（共 2 幕）"), "{body}");
    assert!(
        body.contains("心象二。") && !body.contains("专属信赖邀约"),
        "{body}"
    );
    // 标题含名字的其他代理人档案只列出，不混用。
    assert_eq!(data["otherProfiles"], json!(["测试主角B（测试主角B）"]));
}

#[test]
fn dialogue_scenes_cover_each_kind_and_prefer_full_daily_scenes() {
    let c = corpus();
    let (text, data) = call(&c, json!({"name":HERO,"purpose":"roleplay"})).unwrap();
    // 默认 6 段：信赖邀约 1、随行气泡 1 组块、街区日常 4；按档案行序。
    assert_eq!(
        scene_headings(&data),
        [
            "「长邀约」",
            "街区日常 0",
            "街区日常 2",
            "街区日常 4",
            "街区日常 6",
            "10/15 组"
        ],
        "{text}"
    );
    let scenes = data["dialogueScenes"].as_array().unwrap();
    let lines: Vec<u64> = scenes.iter().map(|s| s["line"].as_u64().unwrap()).collect();
    let mut sorted = lines.clone();
    sorted.sort();
    assert_eq!(lines, sorted);
    // 随行气泡：15 组里等距取 10 组，每组 2 行。
    let bubble = scenes.last().unwrap();
    assert_eq!(bubble["section"], "随行气泡与特别对话（共 15 组）");
    assert_eq!(bubble["lineCount"], 20);
    assert_eq!(bubble["truncated"], false);
    let groups: BTreeSet<String> = bubble["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            l["text"]
                .as_str()
                .unwrap()
                .split('第')
                .next()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(groups.len(), 10, "{groups:?}");
    assert!(
        groups.contains("- 气泡0") && groups.contains("- 气泡13"),
        "{groups:?}"
    );
    // 文本去掉列表前缀与行尾来源键，structuredContent 保留原文。
    assert!(text.contains("】日常0第0句，店长。\n"), "{text}");
    assert!(!text.contains("(`Key_D0_0`)"), "{text}");
    let first_daily = &scenes[1]["lines"][0];
    assert_eq!(
        first_daily["text"],
        "- 【测试主角】日常0第0句，店长。 (`Key_D0_0`)"
    );
    // 行号即档案内行号，可直接读取。
    let n = first_daily["number"].as_u64().unwrap();
    let body = read(
        &c,
        &json!({"docId":data["profile"]["docId"],"offset":n,"limit":1}),
    );
    assert!(body.contains("日常0第0句"), "{body}");
    assert!(
        text.contains("全部档案对话：专属信赖邀约（GalGame事件，共 2 幕） → ik_read("),
        "{text}"
    );
}

#[test]
fn long_scenes_are_cut_at_whole_lines_with_a_continuation_call() {
    let c = corpus();
    let (text, data) = call(&c, json!({"name":HERO,"purpose":"roleplay"})).unwrap();
    let trust = &data["dialogueScenes"][0];
    assert_eq!(trust["heading"], "「长邀约」");
    assert_eq!(trust["lineCount"], 40);
    assert_eq!(trust["lines"].as_array().unwrap().len(), 30);
    assert_eq!(trust["truncated"], true);
    assert!(
        text.contains("（本段 40 行，已列 30 行，续读：ik_read("),
        "{text}"
    );
    let rest = read(&c, &trust["read"]["arguments"]);
    assert!(
        rest.contains("邀约第30句") && rest.contains("邀约第39句"),
        "{rest}"
    );
    assert!(
        !rest.contains("邀约第29句") && !rest.contains("短邀约"),
        "{rest}"
    );
}

#[test]
fn scene_budget_is_redistributed_when_a_kind_is_missing() {
    let c = corpus();
    let (_, one) = call(&c, json!({"name":HERO,"purpose":"roleplay","maxScenes":1})).unwrap();
    assert_eq!(scene_headings(&one), ["街区日常 0"]);
    let (_, two) = call(&c, json!({"name":HERO,"purpose":"roleplay","maxScenes":2})).unwrap();
    assert_eq!(scene_headings(&two), ["街区日常 0", "10/15 组"]);
    // 只有信赖邀约的档案：名额全给邀约。
    let (text, only) = call(&c, json!({"name":"仅邀约","purpose":"roleplay"})).unwrap();
    assert_eq!(
        scene_headings(&only),
        ["「一」", "「二」", "「三」"],
        "{text}"
    );
    // 只有设定、没有对话的档案。
    let (text, bare) = call(&c, json!({"name":"档案角色","purpose":"roleplay"})).unwrap();
    assert!(text.contains("档案中没有对话小节"), "{text}");
    assert!(text.contains("不是已知说话人"), "{text}");
    assert_eq!(bare["storySamples"], json!([]));
    assert_eq!(bare["nextCalls"], json!([]));
    let e = call(&c, json!({"name":HERO,"maxScenes":3})).unwrap_err();
    assert!(e.contains("purpose=roleplay"), "{e}");
}

#[test]
fn profiles_match_by_title_name_or_full_name() {
    let c = corpus();
    // 括号里的全名（忽略空格）同样能找到档案。
    let (_, by_full) = call(&c, json!({"name":"测试全名","purpose":"roleplay"})).unwrap();
    assert_eq!(by_full["profile"]["title"], "测试主角（测试 全名）");
    // 精确名优先于包含匹配。
    let (_, b) = call(&c, json!({"name":"测试主角B","purpose":"roleplay"})).unwrap();
    assert_eq!(b["profile"]["title"], "测试主角B（测试主角B）");
    assert_eq!(b["otherProfiles"], json!([]));
}

#[test]
fn story_samples_spread_across_chapters_and_are_deterministic() {
    let c = corpus();
    let args = json!({"name":HERO,"purpose":"roleplay"});
    let (text, data) = call(&c, args.clone()).unwrap();
    let samples = data["storySamples"].as_array().unwrap();
    assert_eq!(samples.len(), 8, "{text}");
    let chapters: BTreeSet<&str> = samples
        .iter()
        .map(|s| s["groupId"].as_str().unwrap())
        .collect();
    assert_eq!(chapters.len(), 4, "{chapters:?}");
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
    assert_eq!(call(&c, args).unwrap().1, data);
    for s in samples {
        let r = &s["read"]["arguments"];
        assert_eq!(r["docId"], s["docId"]);
        let offset = r["offset"].as_u64().unwrap();
        let line = s["line"]["number"].as_u64().unwrap();
        assert!(offset <= line && line < offset + r["limit"].as_u64().unwrap());
    }
}

#[test]
fn only_reliable_distinct_story_dialogue_of_this_character_is_sampled() {
    let c = corpus();
    let (text, data) = call(
        &c,
        json!({"name":HERO,"purpose":"roleplay","maxResults":40}),
    )
    .unwrap();
    let all = sample_texts(&data);
    assert!(all.iter().all(|t| !t.contains("推断署名")), "{text}");
    assert!(all.iter().all(|t| !t.contains("选项内容")), "{text}");
    assert!(all.iter().all(|t| t != "嗯。"), "{text}");
    assert_eq!(
        all.iter().filter(|t| t.contains("交给我吧")).count(),
        1,
        "{text}"
    );
    for s in data["storySamples"].as_array().unwrap() {
        assert_eq!(s["line"]["speaker"], HERO);
        assert_eq!(s["line"]["kind"], "dialogue");
    }
    // 20 场 × 3 句署名对白 + 第 4 章多出的 1 句；推断署名不计。
    assert_eq!(data["storySpokenLines"], 61);
    assert_eq!(data["storyScenes"], 20);
    // 20 句长回答 + 1 句口头禅 + 1 句「知道了，哲。」
    assert_eq!(data["storyCandidateLines"], 22);
    assert_eq!(all.len(), 22);
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
    // 「知道了，哲。」比其余样本都短，只有按对话对象保底才会被选中。
    let contexts: BTreeSet<&str> = data["storySamples"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["context"]["speaker"].as_str())
        .collect();
    assert!(contexts.contains("铃") && contexts.contains("哲"), "{text}");
    assert!(
        sample_texts(&data).contains(&"知道了，哲。".to_owned()),
        "{text}"
    );
    assert!(text.contains("哲：第四章，哲又开口。\nL"), "{text}");
}

#[test]
fn until_limits_story_lines_but_keeps_the_profile() {
    let c = corpus();
    let (text, data) = call(
        &c,
        json!({"name":HERO,"purpose":"roleplay","until":"ch:Chapter02","maxResults":40}),
    )
    .unwrap();
    assert_eq!(data["until"]["groupId"], "ch:Chapter02");
    assert_eq!(data["storySpokenLines"], 30);
    for s in data["storySamples"].as_array().unwrap() {
        let g = s["groupId"].as_str().unwrap();
        assert!(g == "ch:Chapter01" || g == "ch:Chapter02", "{g}");
    }
    assert_eq!(data["dialogueScenes"].as_array().unwrap().len(), 6);
    assert!(text.contains("截止 ch:Chapter02 测试第2章（含）"), "{text}");
    assert!(text.contains("档案不随截止过滤"), "{text}");
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
fn names_must_identify_one_character() {
    let c = corpus();
    let e = call(&c, json!({"name":"主角","purpose":"roleplay"})).unwrap_err();
    assert!(e.contains("测试主角") && e.contains("测试主角B"), "{e}");
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
    assert!(schema.contains("#/$defs/PersonaScene"), "{schema}");
    assert!(schema.contains("#/$defs/CharacterResult"), "{schema}");
    let input = &character["inputSchema"]["properties"];
    assert_eq!(input["purpose"]["enum"], json!(["overview", "roleplay"]));
    assert!(input["until"].is_object() && input["maxScenes"].is_object());
}
