//! 端到端测试：用 tests/fixtures/sample（带说话人的小型提取器输出）构建语料，
//! 覆盖构建、快照、各工具、JSON-RPC 分发与 HTTP 传输。

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use ik_kernel::config::{Secret, Story};
use ik_story::StoryState;
use ik_story::corpus::{Bundle, Corpus, DocKind, build_from_bundle, build_from_dir};
use ik_story::mcp::{Server, tools};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample")
}

fn corpus() -> Corpus {
    build_from_dir(&fixture()).unwrap()
}

fn call(corpus: &Corpus, tool: &str, args: Value) -> Result<String, String> {
    match tools::call(corpus, tool, Some(args)) {
        Ok(Ok(text)) => Ok(text),
        Ok(Err(e)) => Err(e.0),
        Err(_) => panic!("unknown tool {tool}"),
    }
}

fn ok(corpus: &Corpus, tool: &str, args: Value) -> String {
    call(corpus, tool, args).unwrap_or_else(|e| panic!("{tool} failed: {e}"))
}

/// 从输出里取第一个 doc_ 标识。
fn first_doc_id(text: &str) -> String {
    let at = text.find("doc_").expect("no doc id");
    text[at..at + 16].to_owned()
}

fn release_corpus() -> Corpus {
    let mut bundle = Bundle::from_dir(&fixture()).unwrap();
    bundle.story = [
        json!({"chapter":"Chapter230", "chapter_label":"第三季 第三章 她与她的隐秘往事", "scene":"1", "family":"GalGame", "speaker":"Fairy", "text":"上一章的核心。"}),
        json!({"chapter":"Chapter240", "chapter_label":"第三季 第四章 重返天空的旅程", "scene":"1", "family":"GalGame", "speaker":"Fairy", "text":"天空之城。", "order":1}),
        json!({"chapter":"Chapter240", "chapter_label":"第三季 第四章 重返天空的旅程", "scene":"1", "family":"GalGame", "speaker":"Fairy", "text":"核心就在这里。", "order":2}),
        json!({"chapter":"Chapter240", "chapter_label":"第三季 第四章 重返天空的旅程", "scene":"2", "family":"GalGame", "speaker":"哲", "text":"天空很远。", "order":3}),
        json!({"chapter":"Chapter250", "chapter_label":"未知新章", "scene":"1", "family":"GalGame", "speaker":"Fairy", "text":"未来的核心，增幅3.3%。"}),
    ].iter().map(Value::to_string).collect::<Vec<_>>().join("\n");
    bundle.supplement = None;
    bundle.lore.clear();
    bundle.manifest.as_mut().unwrap()["source_version"] = json!("3.3.0");
    build_from_bundle(&bundle).unwrap()
}

#[test]
fn search_usability_default_fallback_keeps_filters_and_coverage_order() {
    let c = release_corpus();
    let out = ok(
        &c,
        "ik_search",
        json!({"keyword":"天空 核心 秘密", "path":"Chapter240"}),
    );
    assert!(out.contains("部分匹配"), "{out}");
    assert!(out.contains("命中 2 篇"), "{out}");
    assert!(out.contains("未命中关键词：秘密"), "{out}");
    assert!(
        out.find("核心就在这里").unwrap() < out.find("天空很远").unwrap(),
        "{out}"
    );
    assert!(
        !out.contains("上一章的核心") && !out.contains("未来的核心"),
        "{out}"
    );
    let strict = ok(
        &c,
        "ik_search",
        json!({"keyword":"天空 核心 秘密", "path":"Chapter240", "allowPartial":false}),
    );
    assert!(strict.contains("命中 0 篇"), "{strict}");
    let filtered = ok(
        &c,
        "ik_search",
        json!({"keyword":"天空 核心 秘密", "path":"Chapter240", "speaker":"Fairy"}),
    );
    assert!(
        filtered.contains("命中 1 篇") && !filtered.contains("天空很远"),
        "{filtered}"
    );
    let complete = ok(
        &c,
        "ik_search",
        json!({"keyword":"天空 核心", "path":"Chapter240"}),
    );
    assert!(
        complete.contains("命中 1 篇") && !complete.contains("部分匹配"),
        "{complete}"
    );
    let doc = first_doc_id(&complete);
    let local = ok(&c, "ik_search", json!({"docId":doc,"keyword":"天空 秘密"}));
    assert!(
        local.contains("命中 1 行") && local.contains("未命中关键词：秘密"),
        "{local}"
    );
}

#[test]
fn search_usability_release_aliases_cover_catalog_read_and_keywords() {
    let c = release_corpus();
    for alias in ["3.3", "3.3.0", "v3.3", "3.3版本", "版本 3.3", "３．３版本"] {
        let out = ok(&c, "ik_catalog", json!({"path":alias}));
        assert!(
            out.contains("ch:Chapter240") && out.contains("版本别名"),
            "{out}"
        );
        assert!(!out.contains("上一章的核心"), "{out}");
        let read = ok(&c, "ik_read", json!({"docId":alias,"limit":1}));
        assert!(
            read.contains("天空之城") && read.contains("版本别名"),
            "{read}"
        );
        assert!(read.contains("limit=1"), "{read}");
    }
    let root = ok(&c, "ik_catalog", json!({}));
    assert!(
        root.contains("版本别名：3.2") && root.contains("版本别名：3.3"),
        "{root}"
    );
    let titles = ok(&c, "ik_catalog", json!({"keyword":"3.3版本"}));
    assert!(
        titles.contains("ch:Chapter240") && titles.contains("版本别名"),
        "{titles}"
    );
    let out = ok(&c, "ik_search", json!({"keyword":"3.3版本 核心"}));
    assert!(
        out.contains("命中 1 篇") && out.contains("核心就在这里"),
        "{out}"
    );
    assert!(
        out.contains("该版新增主线")
            && !out.contains("上一章的核心")
            && !out.contains("未来的核心"),
        "{out}"
    );
    let all = ok(&c, "ik_search", json!({"keyword":"v3.3.0"}));
    assert!(
        all.contains("命中 2 篇") && all.contains("版本别名"),
        "{all}"
    );
    let older = ok(&c, "ik_search", json!({"keyword":"3.2 核心"}));
    assert!(
        older.contains("上一章的核心") && !older.contains("核心就在这里"),
        "{older}"
    );
    let literal = ok(&c, "ik_search", json!({"keyword":"3.3%"}));
    assert!(
        literal.contains("未来的核心") && !literal.contains("版本别名"),
        "{literal}"
    );
}

#[test]
fn search_usability_release_constraints_are_never_relaxed_or_guessed() {
    let c = release_corpus();
    let out = ok(&c, "ik_search", json!({"keyword":"3.3 天空 核心 秘密"}));
    assert!(
        out.contains("部分匹配") && out.contains("命中 2 篇"),
        "{out}"
    );
    assert!(
        !out.contains("上一章的核心") && !out.contains("未来的核心"),
        "{out}"
    );
    assert!(
        call(
            &c,
            "ik_search",
            json!({"keyword":"3.3 核心", "path":"Chapter230"})
        )
        .is_err()
    );
    assert!(call(&c, "ik_search", json!({"keyword":"3.2 3.3 核心"})).is_err());
    let unavailable = call(&corpus(), "ik_search", json!({"keyword":"3.3 核心"})).unwrap_err();
    assert!(
        unavailable.contains("Chapter240") && unavailable.contains("当前语料"),
        "{unavailable}"
    );
    let unknown = call(&c, "ik_catalog", json!({"path":"3.4版本"})).unwrap_err();
    assert!(
        unknown.contains("没有已确认") && unknown.contains("版本"),
        "{unknown}"
    );
    let filtered = ok(
        &c,
        "ik_search",
        json!({"keyword":"3.3 天空 核心", "scope":"lore"}),
    );
    assert!(filtered.contains("命中 0 篇"), "{filtered}");
    let no_hits = ok(&c, "ik_search", json!({"keyword":"完全不存在 仍不存在"}));
    assert!(
        no_hits.contains("命中 0 篇") && no_hits.contains("没有命中"),
        "{no_hits}"
    );
    let truncated = ok(
        &c,
        "ik_search",
        json!({"keyword":"天空 核心 一 二 三 四 五 六 七 3.3版本"}),
    );
    assert!(
        truncated.contains("前 8") && truncated.contains("版本别名"),
        "{truncated}"
    );
    assert!(
        !truncated.contains("上一章的核心") && !truncated.contains("未来的核心"),
        "{truncated}"
    );
    let doc = first_doc_id(&ok(&c, "ik_search", json!({"keyword":"3.2 核心"})));
    assert!(call(&c, "ik_search", json!({"keyword":"3.3 核心","docId":doc})).is_err());
    assert!(
        call(
            &c,
            "ik_search",
            json!({"keyword":"核心","path":"3.3版本","docId":doc})
        )
        .is_err()
    );
    assert!(call(&c, "ik_catalog", json!({"path":"v3.3.1"})).is_err());
}

#[test]
fn review_numeric_arguments() {
    let c = corpus();
    assert!(
        call(
            &c,
            "ik_catalog",
            json!({"path":"ch:Chapter00", "offset":1.5})
        )
        .is_err()
    );
    assert!(call(&c, "ik_catalog", json!({"offset":1e100})).is_err());
    let out = ok(
        &c,
        "ik_catalog",
        json!({"path":"ch:Chapter00", "offset":usize::MAX}),
    );
    assert!(out.contains("显示"));
    let out = ok(
        &c,
        "ik_catalog",
        json!({"keyword":"序章", "offset":usize::MAX}),
    );
    assert!(out.contains("显示"));
}

#[test]
fn review_read_continuation_preferences() {
    let c = corpus();
    let out = ok(
        &c,
        "ik_read",
        json!({"docId":"ch:Chapter00", "limit":1, "maxChars":800, "lineNumbers":false, "includeSources":true}),
    );
    let continuation = out.split("继续：").nth(1).unwrap();
    assert!(continuation.contains("limit=1"));
    assert!(continuation.contains("maxChars=800"));
    assert!(continuation.contains("lineNumbers=false"));
    assert!(continuation.contains("follow=true"));
    assert!(continuation.contains("includeSources=true"));
}

#[test]
fn review_plain_bundle_limit() {
    let bundle = Bundle::from_dir(&fixture()).unwrap();
    let bytes = serde_json::to_vec(&bundle).unwrap();
    assert!(Bundle::decode(&bytes, bytes.len() - 1).is_err());
    assert!(Bundle::decode(&bytes, bytes.len()).is_ok());
}

#[test]
fn review_read_long_next_header() {
    let mut bundle = Bundle::from_dir(&fixture()).unwrap();
    bundle.story = [
        json!({"chapter":"Chapter00", "scene":"1", "family":"GalGame", "text":"第一行", "order":1}),
        json!({"chapter":"Chapter00", "scene":"2", "scene_title":"很长的标题".repeat(250), "family":"GalGame", "text":"第二行", "order":2}),
    ].iter().map(Value::to_string).collect::<Vec<_>>().join("\n");
    bundle.supplement = None;
    bundle.lore.clear();
    let c = build_from_bundle(&bundle).unwrap();
    let out = ok(
        &c,
        "ik_read",
        json!({"docId":"ch:Chapter00", "maxChars":1000}),
    );
    assert!(!out.contains("第 1-0 行"));
    assert!(!out.contains("第二行"));
    let next = first_doc_id(out.split("继续：").nth(1).unwrap());
    let out = ok(&c, "ik_read", json!({"docId":next, "maxChars":1000}));
    assert!(out.contains("第二行"));
}

#[test]
fn review_in_doc_query_overflow_is_explicit() {
    let c = corpus();
    let id = c.doc_id(0);
    let out = ok(
        &c,
        "ik_search",
        json!({"docId":id,"keyword":"一 二 三 四 五 六 七 八 九"}),
    );
    assert!(out.contains("前 8"));
}

#[test]
fn review_legacy_fractional_chapter_order() {
    let mut bundle = Bundle::from_dir(&fixture()).unwrap();
    bundle.story = [
        json!({"chapter":"Chapter01", "chapter_order":2, "scene":"1", "family":"GalGame", "text":"甲"}),
        json!({"chapter":"Chapter015", "chapter_order":2.5, "scene":"1", "family":"GalGame", "text":"乙"}),
        json!({"chapter":"Chapter02", "chapter_order":3, "scene":"1", "family":"GalGame", "text":"丙"}),
    ].iter().map(Value::to_string).collect::<Vec<_>>().join("\n");
    bundle.supplement = None;
    bundle.lore.clear();
    let c = build_from_bundle(&bundle).unwrap();
    assert_eq!(
        c.groups.iter().map(|g| g.id.as_str()).collect::<Vec<_>>(),
        ["ch:Chapter01", "ch:Chapter015", "ch:Chapter02"]
    );
    bundle.manifest.as_mut().unwrap()["chapter_order_scale"] = json!(10);
    assert!(build_from_bundle(&bundle).is_err());
}

#[test]
fn review_metadata_search_consistency() {
    let mut bundle = Bundle::from_dir(&fixture()).unwrap();
    bundle.story = json!({"chapter":"Chapter00", "scene":"1", "scene_title":"标题词", "family":"GalGame", "text":"正文词"}).to_string();
    bundle.supplement = None;
    bundle.lore.clear();
    let c = build_from_bundle(&bundle).unwrap();
    let global = ok(&c, "ik_search", json!({"keyword":"标题词"}));
    assert!(global.contains("读上下文：ik_read"));
    let local = ok(
        &c,
        "ik_search",
        json!({"docId":c.doc_id(0), "keyword":"标题词 正文词"}),
    );
    assert!(local.contains("命中 1 行"));
    assert!(local.contains("标题/出场命中"));
}

#[test]
fn review_ambiguous_group_is_rejected() {
    let mut c = corpus();
    c.groups[0].label = "重复标签".into();
    c.groups[1].label = "重复标签".into();
    assert!(call(&c, "ik_catalog", json!({"path":"重复标签"})).is_err());
    assert!(call(&c, "ik_catalog", json!({"path":"标签"})).is_err());
    assert!(call(&c, "ik_catalog", json!({"path":"ch:Chapter00"})).is_ok());
}

#[test]
fn review_unreadable_manifest_is_not_silently_ignored() {
    let dir = std::env::temp_dir().join(format!("ik-story-manifest-review-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("manifest.json")).unwrap();
    std::fs::write(dir.join("story.jsonl"), "{}\n").unwrap();
    let result = Bundle::from_dir(&dir);
    std::fs::remove_dir_all(dir).unwrap();
    assert!(result.is_err());
}

#[test]
fn review_identity_match_is_labeled() {
    let mut bundle = Bundle::from_dir(&fixture()).unwrap();
    bundle.story = json!({"chapter":"Chapter00", "scene":"1", "family":"GalGame", "text":"测试台词", "speaker":"服装甲", "speaker_id":"本人"}).to_string();
    bundle.supplement = None;
    bundle.lore.clear();
    let c = build_from_bundle(&bundle).unwrap();
    let global = ok(&c, "ik_search", json!({"keyword":"本人"}));
    let local = ok(
        &c,
        "ik_search",
        json!({"docId":c.doc_id(0), "keyword":"本人"}),
    );
    assert!(global.contains("说话人名/身份匹配：本人"));
    assert!(local.contains("说话人名/身份匹配：本人"));
}

#[test]
fn builds_docs_groups_and_speakers() {
    let c = corpus();
    let story: Vec<_> = c.docs.iter().filter(|d| d.kind == DocKind::Story).collect();
    assert_eq!(story.len(), 4);
    assert_eq!(
        c.docs
            .iter()
            .filter(|d| d.kind == DocKind::Supplement)
            .count(),
        1
    );
    // 代理人档案按三级标题切成两篇；委托档案：引言 + 1 条 + 重复条目只保留一份；README 不收录。
    let lore: Vec<&str> = c
        .docs
        .iter()
        .filter(|d| d.kind == DocKind::Lore)
        .map(|d| d.title.as_str())
        .collect();
    assert_eq!(
        lore,
        vec![
            "安比（安比·德玛拉）",
            "妮可（妮可·德玛拉）",
            "绳网（Inter-Knot）委托档案",
            "【找到妮可】（委托编号：1）",
            "【重复条目】（委托编号：2）"
        ]
    );
    // 章节按游戏顺序，设定在后。
    let ids: Vec<&str> = c.groups.iter().map(|g| g.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "ch:Chapter00",
            "ch:Chapter01",
            "lore:agents",
            "lore:interknot"
        ]
    );
    // 同章内主线在前、补充在后。
    let ch00 = &c.groups[0];
    assert_eq!(
        c.docs[*ch00.docs.last().unwrap() as usize].kind,
        DocKind::Supplement
    );
    assert!((c.meta.speaker_coverage - 11.0 / 14.0).abs() < 1e-6);
    assert_eq!(c.meta.source_version, "3.2.0");
    // 每行都能映射回所在文档。
    for (i, d) in c.docs.iter().enumerate() {
        for l in d.lines() {
            assert_eq!(c.doc_of_line(l as u32), i);
        }
    }
}

#[test]
fn bundle_round_trip_and_validation() {
    let c = corpus();
    let bundle = Bundle::from_dir(&fixture()).unwrap();
    // lore 根目录的 README.md 也在包里，但构建时不收录（见 builds_docs_groups_and_speakers）。
    assert!(bundle.lore.contains_key("README.md"));
    assert!(bundle.lore.contains_key("agents/01_狡兔屋.md"));

    let gz = bundle.encode_gzip().unwrap();
    assert_eq!(&gz[..2], &[0x1f, 0x8b]);
    let decoded = Bundle::decode(&gz, 10_000_000).unwrap();
    let rebuilt = build_from_bundle(&decoded).unwrap();
    assert_eq!(rebuilt.text, c.text);
    assert_eq!(rebuilt.docs.len(), c.docs.len());
    let a = ok(&c, "ik_search", json!({ "keyword": "狡兔屋" }));
    let b = ok(&rebuilt, "ik_search", json!({ "keyword": "狡兔屋" }));
    assert_eq!(a, b, "同一份数据，文档 id 与结果都应一致");

    // 未压缩的 JSON 也接受。
    let plain = serde_json::to_vec(&bundle).unwrap();
    assert!(Bundle::decode(&plain, 10_000_000).is_ok());

    // 解压后超限（防压缩炸弹）、坏的 gzip、格式版本不对、主线为空：都拒绝。
    let err = Bundle::decode(&gz, 100).unwrap_err().to_string();
    assert!(err.contains("上限"), "{err}");
    let mut broken = gz.clone();
    broken.truncate(gz.len() / 2);
    assert!(Bundle::decode(&broken, 10_000_000).is_err());
    let mut future = bundle.clone();
    future.format = 99;
    let err = Bundle::decode(&serde_json::to_vec(&future).unwrap(), 10_000_000)
        .unwrap_err()
        .to_string();
    assert!(err.contains("格式"), "{err}");
    let mut empty = bundle.clone();
    empty.story = " \n".to_owned();
    assert!(Bundle::decode(&serde_json::to_vec(&empty).unwrap(), 10_000_000).is_err());
    let mut bad_line = bundle;
    bad_line.story.push_str("{not json}\n");
    let err = build_from_bundle(&bad_line).unwrap_err().to_string();
    assert!(err.contains("story.jsonl 第 15 行"), "{err}");
}

#[test]
fn search_and_semantics_with_speaker_and_scope() {
    let c = corpus();
    let out = ok(&c, "ik_search", json!({ "keyword": "狡兔屋 妮可" }));
    assert!(out.contains("命中"), "{out}");
    assert!(!out.contains("部分匹配"));
    assert!(
        out.contains("妮可：安比，交给你了！狡兔屋的招牌可不能砸。"),
        "{out}"
    );

    // 说话人过滤：只保留妮可本人的台词。
    let out = ok(
        &c,
        "ik_search",
        json!({ "keyword": "狡兔屋", "speaker": "妮可" }),
    );
    assert!(out.contains("妮可：安比"), "{out}");
    assert!(!out.contains("哲：绳网上"), "{out}");

    // 范围：设定。
    let out = ok(
        &c,
        "ik_search",
        json!({ "keyword": "赤字", "scope": "lore" }),
    );
    assert!(out.contains("妮可（妮可·德玛拉）"), "{out}");
    assert!(!out.contains("主线 ›"), "{out}");

    // 严格 AND 可显式要求；默认回退必须标注部分匹配。
    let out = ok(
        &c,
        "ik_search",
        json!({ "keyword": "猫又 赫利俄斯", "allowPartial":false }),
    );
    assert!(out.contains("命中 0 篇"), "{out}");
    assert!(!out.contains("以下按覆盖"), "{out}");
    let out = ok(
        &c,
        "ik_search",
        json!({ "keyword": "猫又 赫利俄斯", "allowPartial": true }),
    );
    assert!(out.contains("部分匹配"), "{out}");
    assert!(out.contains("未命中关键词"), "{out}");

    // 全角、大小写折叠。
    let out = ok(&c, "ik_search", json!({ "keyword": "ＦＡＩＲＹ" }));
    assert!(out.contains("Fairy：检测到以太活性异常升高。"), "{out}");

    // 只按说话人列出台词。
    let out = ok(&c, "ik_search", json!({ "speaker": "零号·安比" }));
    assert!(out.contains("零号·安比：这里交给我。"), "{out}");

    let err = call(
        &c,
        "ik_search",
        json!({ "keyword": "x", "speaker": "不存在的人" }),
    )
    .unwrap_err();
    assert!(err.contains("没有名为"), "{err}");
    let err = call(&c, "ik_search", json!({ "keyword": "x", "scope": "bogus" })).unwrap_err();
    assert!(err.contains("scope"), "{err}");
    let out = ok(&c, "ik_search", json!({ "keyword": "完全不存在的词组" }));
    assert!(out.contains("没有命中"), "{out}");
}

#[test]
fn search_within_doc_and_path() {
    let c = corpus();
    let found = ok(&c, "ik_search", json!({ "keyword": "赤字" }));
    let doc = first_doc_id(&found);
    let out = ok(
        &c,
        "ik_search",
        json!({ "docId": doc, "keyword": "狡兔屋" }),
    );
    assert!(out.starts_with("[ik_search] 篇内检索"), "{out}");
    assert!(out.contains("命中 2 行"), "{out}");

    let out = ok(
        &c,
        "ik_search",
        json!({ "keyword": "零号", "path": "第一季 第一章" }),
    );
    assert!(out.contains("零号空洞"), "{out}");
    let out = ok(
        &c,
        "ik_search",
        json!({ "keyword": "妮可", "path": "Chapter01" }),
    );
    assert!(out.contains("没有命中"), "{out}");
}

#[test]
fn search_context_is_ordered_unique_and_bounded() {
    let c = corpus();
    let out = ok(
        &c,
        "ik_search",
        json!({
            "keyword": "妮可", "path": "Chapter00", "contextLines": 3,
        }),
    );
    let mut numbers = Vec::new();
    for line in out.lines() {
        if line.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            assert!(
                numbers.windows(2).all(|pair: &[usize]| pair[0] < pair[1]),
                "{out}"
            );
            numbers.clear();
        }
        let line = line.trim().trim_start_matches("上下文 ");
        if let Some(number) = line
            .strip_prefix('L')
            .and_then(|s| s.split_whitespace().next())
            .and_then(|s| s.parse::<usize>().ok())
        {
            numbers.push(number);
        }
    }
    assert!(numbers.len() > 1, "{out}");
    assert!(numbers.windows(2).all(|pair| pair[0] < pair[1]), "{out}");
    assert!(!out.contains("offset=行号-5"), "{out}");
    let out = ok(
        &c,
        "ik_search",
        json!({ "keyword": "狡兔屋", "speaker": "妮可" }),
    );
    assert!(
        !out.contains("上下文 L"),
        "speaker filter defaults to hits only: {out}"
    );
}

#[test]
fn search_within_doc_requires_all_terms_in_strict_mode() {
    let c = corpus();
    let doc = c.doc_id(0);
    let out = ok(
        &c,
        "ik_search",
        json!({ "docId": doc, "keyword": "录像店 赫利俄斯", "allowPartial":false }),
    );
    assert!(out.contains("命中 0 行"), "{out}");
    let out = ok(
        &c,
        "ik_search",
        json!({ "docId": doc, "keyword": "录像店 赫利俄斯", "allowPartial": true }),
    );
    assert!(out.contains("部分匹配"), "{out}");
    assert!(out.contains("未命中关键词：赫利俄斯"), "{out}");
    // Document AND includes hits on separate lines; it never requires a single-line AND.
    let out = ok(
        &c,
        "ik_search",
        json!({ "docId": doc, "keyword": "录像店 丰厚" }),
    );
    assert!(out.contains("命中 2 行"), "{out}");
    assert!(out.contains("L1 ") && out.contains("L6 "), "{out}");
}

#[test]
fn sources_and_identity_survive_ingestion_and_continuation() {
    let mut bundle = Bundle::from_dir(&fixture()).unwrap();
    let mut rows: Vec<Value> = bundle
        .story
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    for row in &mut rows {
        row["order_source"] = json!("archive_table");
        row["chronology_status"] = json!("unknown");
        if row["speaker"] == "哲" {
            row["speaker"] = json!("道士哲");
            row["speaker_id"] = json!("哲");
        }
    }
    bundle.story = rows
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    let c = build_from_bundle(&bundle).unwrap();
    let original = corpus();
    assert_eq!(
        c.doc_id(0),
        original.doc_id(0),
        "metadata must not renumber docs"
    );
    assert_eq!(c.lines.len(), c.line_sources.len());
    let out = ok(
        &c,
        "ik_read",
        json!({ "docId": "ch:Chapter00", "includeSources": "true", "limit": 6 }),
    );
    assert!(
        out.contains("sourceKey=Main_GalGame_Chapter00_1000001_001"),
        "{out}"
    );
    assert!(out.contains("lineId=Chapter00_1000001_1"), "{out}");
    assert!(out.contains("顺序来源：archive_table"), "{out}");
    assert!(
        out.contains("follow=true, limit=6, includeSources=true)"),
        "next-doc continuation: {out}"
    );
    let out = ok(&c, "ik_search", json!({ "speaker": "哲" }));
    assert!(out.contains("道士哲："), "{out}");
    let out = ok(&c, "ik_character", json!({ "name": "哲" }));
    assert!(out.contains("台词 3 句"), "{out}");
    assert!(out.contains("目录首见"), "{out}");
    // Optional fields are absent in legacy bundles; reading remains supported.
    let out = ok(&original, "ik_read", json!({ "docId": original.doc_id(0) }));
    assert!(!out.contains("sourceKey="), "{out}");
}

#[test]
fn read_budget_counts_fragment_headers() {
    let mut bundle = Bundle::from_dir(&fixture()).unwrap();
    let rows: Vec<String> = (0..100)
        .map(|i| {
            json!({
                "chapter": "Comic", "chapter_label": "动态漫画", "chapter_order": 9990,
                "scene": format!("fragment_{i}"), "family": "Comic", "text": "片段。",
                "context_status": "unlinked", "order": i,
            })
            .to_string()
        })
        .collect();
    bundle.supplement = Some(rows.join("\n"));
    let c = build_from_bundle(&bundle).unwrap();
    let out = ok(
        &c,
        "ik_read",
        json!({ "docId": "ch:Comic", "maxChars": 5000 }),
    );
    assert!(
        out.chars().count() < 5500,
        "headers exceeded budget: {}",
        out.chars().count()
    );
    assert!(
        out.contains("下一篇（目录相邻"),
        "must offer continuation: {out}"
    );
}

#[test]
fn character_coappearance_merges_forms_once_per_scene() {
    let mut bundle = Bundle::from_dir(&fixture()).unwrap();
    let rows: Vec<String> = [
        ("妮可", "妮可"),
        ("哲", "哲"),
        ("学生哲", "哲"),
        ("道士哲", "哲"),
    ]
    .iter()
    .enumerate()
    .map(|(i, (speaker, identity))| {
        json!({
            "chapter": "Chapter00", "scene": "test", "family": "GalGame",
            "speaker": speaker, "speaker_id": identity, "text": "同场台词", "order": i,
        })
        .to_string()
    })
    .collect();
    bundle.story = rows.join("\n");
    bundle.supplement = None;
    let c = build_from_bundle(&bundle).unwrap();
    let out = ok(&c, "ik_character", json!({ "name": "妮可" }));
    assert!(out.contains("常同场角色（共同出场场景数）：哲(1)"), "{out}");
    assert!(
        !out.contains("学生哲(1)") && !out.contains("道士哲(1)"),
        "{out}"
    );
}

#[test]
fn read_pages_and_follows_across_docs() {
    let c = corpus();
    // 分组 id 从章首开始并默认 follow，跨场景读到章末。
    let out = ok(
        &c,
        "ik_read",
        json!({ "docId": "ch:Chapter00", "maxChars": 20000 }),
    );
    assert!(out.contains("L2 铃：哥哥，今天也有委托吗？"), "{out}");
    assert!(out.contains("L4 【选项】接下委托"), "{out}");
    assert!(out.contains("L1 六分街，Random Play 录像店。"), "{out}");
    assert!(
        out.contains("## doc_"),
        "should follow into next doc: {out}"
    );
    assert!(
        out.contains("赫利俄斯机关的遗产"),
        "supplement comes last: {out}"
    );
    assert!(
        out.contains("目录末尾") && out.contains("ik_coverage"),
        "{out}"
    );
    assert!(out.contains("下一组：ch:Chapter01"), "{out}");

    // 行数上限 → 给出精确续读游标。
    let out = ok(
        &c,
        "ik_read",
        json!({ "docId": "ch:Chapter00", "limit": 2 }),
    );
    assert!(out.contains("第 1-2 行"), "{out}");
    assert!(out.contains("offset=3, follow=true"), "{out}");

    // 单篇读完给出下一篇。
    let first = first_doc_id(&out);
    let out = ok(&c, "ik_read", json!({ "docId": first, "offset": 6 }));
    assert!(out.contains("本篇完。下一篇"), "{out}");
    assert!(out.contains("L6 妮可：法厄同"), "{out}");

    let out = ok(
        &c,
        "ik_read",
        json!({ "docId": first, "lineNumbers": "false", "limit": "1" }),
    );
    assert!(out.contains("\n六分街"), "{out}");

    let err = call(&c, "ik_read", json!({ "docId": first, "offset": 99 })).unwrap_err();
    assert!(err.contains("超出范围"), "{err}");
    let err = call(&c, "ik_read", json!({ "docId": "doc_000000000000" })).unwrap_err();
    assert!(err.contains("找不到文档"), "{err}");
    let err = call(&c, "ik_read", json!({})).unwrap_err();
    assert!(err.contains("docId"), "{err}");
}

#[test]
fn catalog_root_group_and_doc() {
    let c = corpus();
    let root = ok(&c, "ik_catalog", json!({}));
    assert!(root.contains("ch:Chapter00 序章 | 3 场景"), "{root}");
    assert!(root.contains("lore:agents 代理人档案 | 2 篇"), "{root}");
    assert!(root.contains("说话人覆盖 79%"), "{root}");

    let group = ok(&c, "ik_catalog", json!({ "path": "序章" }));
    assert!(group.contains("#### 任务：开张大吉"), "{group}");
    assert!(
        group.contains("录像店的早晨 · 6句 · 逐句署名：铃、哲、妮可"),
        "{group}"
    );
    assert!(group.contains("### 补充 · 过场字幕"), "{group}");

    let filtered = ok(&c, "ik_catalog", json!({ "keyword": "妮可" }));
    assert!(filtered.contains("妮可（妮可·德玛拉）"), "{filtered}");
    assert!(filtered.contains("主线聊天·妮可"), "{filtered}");

    let doc = first_doc_id(&group);
    let card = ok(&c, "ik_catalog", json!({ "path": doc }));
    assert!(
        card.contains("所属：ch:Chapter00 序章（第 1/4 篇）"),
        "{card}"
    );
    assert!(card.contains("下一篇（目录相邻）："), "{card}");

    let paged = ok(
        &c,
        "ik_catalog",
        json!({ "path": "ch:Chapter00", "maxResults": 1 }),
    );
    assert!(
        paged.contains("还有 3 篇：ik_catalog(path=\"ch:Chapter00\", offset=1)"),
        "{paged}"
    );

    let err = call(&c, "ik_catalog", json!({ "path": "不存在的章节" })).unwrap_err();
    assert!(err.contains("找不到分组"), "{err}");
}

#[test]
fn character_profile() {
    let c = corpus();
    let out = ok(&c, "ik_character", json!({ "name": "安比" }));
    assert!(out.contains("匹配说话人：安比"), "{out}");
    assert!(
        out.contains("相关名称（未合并统计）：零号·安比（1 句）"),
        "{out}"
    );
    assert!(out.contains("安比（安比·德玛拉）"), "{out}");
    assert!(out.contains("ch:Chapter00 序章 | 场景 1 · 台词 1"), "{out}");
    assert!(
        out.contains("常同场角色（共同出场场景数）：Fairy(1)、妮可(1)") || out.contains("妮可(1)"),
        "{out}"
    );

    let out = ok(&c, "ik_character", json!({ "name": "赫利俄斯" }));
    assert!(out.contains("不是已知说话人"), "{out}");
    let err = call(&c, "ik_character", json!({ "name": "查无此人" })).unwrap_err();
    assert!(err.contains("没有找到"), "{err}");
}

#[test]
fn guide_modes() {
    let c = corpus();
    assert!(ok(&c, "ik_guide", json!({ "mode": "summary" })).contains("分层压缩"));
    assert!(ok(&c, "ik_guide", json!({ "mode": "查证" })).contains("原文查证"));
    assert!(
        call(&c, "ik_guide", json!({ "mode": "x" }))
            .unwrap_err()
            .contains("可选")
    );
}

struct Rpc {
    server: Server,
    corpus: Corpus,
}

impl Rpc {
    fn handle(&self, body: &[u8]) -> Option<Vec<u8>> {
        self.server.handle(&self.corpus, body)
    }
}

fn server() -> Rpc {
    Rpc {
        server: Server::new().unwrap(),
        corpus: corpus(),
    }
}

fn rpc(server: &Rpc, msg: Value) -> Value {
    let out = server
        .handle(msg.to_string().as_bytes())
        .expect("expected a response");
    serde_json::from_slice(&out).unwrap()
}

#[test]
fn jsonrpc_lifecycle() {
    let s = server();
    let init = rpc(
        &s,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}),
    );
    assert_eq!(init["id"], 1);
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(init["result"]["serverInfo"]["name"], "绳网档案");
    assert_eq!(init["result"]["serverInfo"]["title"], "绳网档案");
    assert!(
        init["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("ik_search")
    );

    // 未知版本 → 回应最新版本。
    let init = rpc(
        &s,
        json!({"jsonrpc":"2.0","id":"a","method":"initialize","params":{"protocolVersion":"1999-01-01"}}),
    );
    assert_eq!(init["id"], "a");
    assert_eq!(
        init["result"]["protocolVersion"],
        ik_story::mcp::PROTOCOL_VERSIONS[0]
    );

    assert!(
        s.handle(br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
            .is_none()
    );
    assert_eq!(
        rpc(&s, json!({"jsonrpc":"2.0","id":2,"method":"ping"}))["result"],
        json!({})
    );

    let list = rpc(&s, json!({"jsonrpc":"2.0","id":3,"method":"tools/list"}));
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        vec![
            "ik_catalog",
            "ik_search",
            "ik_read",
            "ik_character",
            "ik_guide",
            "ik_coverage"
        ]
    );
    for t in list["result"]["tools"].as_array().unwrap() {
        assert_eq!(t["inputSchema"]["type"], "object");
        assert_eq!(t["annotations"]["readOnlyHint"], true);
    }

    let res = rpc(
        &s,
        json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"ik_search","arguments":{"keyword":"狡兔屋"}}}),
    );
    assert_eq!(res["result"]["isError"], false);
    assert!(
        res["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .starts_with("[ik_search]")
    );

    // 工具执行错误 → isError，而不是协议错误。
    let res = rpc(
        &s,
        json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"ik_read","arguments":{"docId":"doc_bad"}}}),
    );
    assert_eq!(res["result"]["isError"], true);
    let res = rpc(
        &s,
        json!({"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"ik_read","arguments":{"offset":"abc","docId":"x"}}}),
    );
    assert_eq!(res["result"]["isError"], true);
    assert!(
        res["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("参数错误")
    );

    let res = rpc(
        &s,
        json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"nope"}}),
    );
    assert_eq!(res["error"]["code"], -32602);
    let res = rpc(&s, json!({"jsonrpc":"2.0","id":8,"method":"bogus/method"}));
    assert_eq!(res["error"]["code"], -32601);

    let res: Value = serde_json::from_slice(&s.handle(b"{not json").unwrap()).unwrap();
    assert_eq!(res["error"]["code"], -32700);
    assert_eq!(res["id"], Value::Null);

    let prompts = rpc(&s, json!({"jsonrpc":"2.0","id":9,"method":"prompts/list"}));
    assert_eq!(prompts["result"]["prompts"].as_array().unwrap().len(), 4);
    let p = rpc(
        &s,
        json!({"jsonrpc":"2.0","id":10,"method":"prompts/get","params":{"name":"chapter_summary","arguments":{"chapter":"序章"}}}),
    );
    assert!(
        p["result"]["messages"][0]["content"]["text"]
            .as_str()
            .unwrap()
            .contains("「序章」")
    );
    let p = rpc(
        &s,
        json!({"jsonrpc":"2.0","id":11,"method":"prompts/get","params":{"name":"chapter_summary","arguments":{}}}),
    );
    assert_eq!(p["error"]["code"], -32602);
}

#[test]
fn jsonrpc_batch() {
    let s = server();
    let body = json!([
        {"jsonrpc":"2.0","id":1,"method":"ping"},
        {"jsonrpc":"2.0","method":"notifications/initialized"},
        {"jsonrpc":"2.0","id":2,"method":"tools/list"}
    ]);
    let out: Value =
        serde_json::from_slice(&s.handle(body.to_string().as_bytes()).unwrap()).unwrap();
    let arr = out.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["id"], 1);
    assert_eq!(arr[1]["id"], 2);
    assert!(
        s.handle(br#"[{"jsonrpc":"2.0","method":"notifications/initialized"}]"#)
            .is_none()
    );
}

/// 空密钥表示公开访问。
fn app(access_key: &str) -> axum::Router {
    let config = Story {
        access_key: Secret::new(access_key),
        public: access_key.is_empty(),
        ..Default::default()
    };
    let state = StoryState::with_corpus(corpus(), &config).unwrap();
    let (router, _doc) = ik_story::mcp_router().split_for_parts();
    router.with_state(state)
}

/// 不会被连接的数据库（这些用例走本地语料或根本不加载）。
fn unused_db() -> (sqlx::PgPool, std::sync::Arc<ik_kernel::Snowflake>) {
    let db = sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(std::time::Duration::from_millis(200))
        .connect_lazy("postgres://x:x@127.0.0.1:1/x")
        .unwrap();
    (
        db,
        std::sync::Arc::new(ik_kernel::Snowflake::new(1).unwrap()),
    )
}

async fn send(
    app: axum::Router,
    req: Request<Body>,
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let body = resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    (status, headers, body)
}

fn post() -> axum::http::request::Builder {
    Request::post("/v1/story/mcp")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
}

#[tokio::test]
async fn http_endpoint() {
    let app = app("");
    let init = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}});
    let (status, headers, body) = send(
        app.clone(),
        post().body(Body::from(init.to_string())).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["content-type"], "application/json");
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["result"]["protocolVersion"], "2025-06-18");

    let note = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
    let (status, _, body) = send(
        app.clone(),
        post().body(Body::from(note.to_string())).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert!(body.is_empty());

    // 无状态：没有推送流，也没有会话可删。
    let (status, _, _) = send(
        app.clone(),
        Request::get("/v1/story/mcp").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    let (status, _, _) = send(
        app.clone(),
        Request::delete("/v1/story/mcp")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);

    let (status, _, body) = send(
        app.clone(),
        post()
            .header("mcp-protocol-version", "1999-01-01")
            .body(Body::from(init.to_string()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["code"], "MCP_PROTOCOL_UNSUPPORTED");

    let call = json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"ik_search","arguments":{"keyword":"狡兔屋"}}});
    let (status, _, body) = send(
        app,
        post()
            .header("mcp-protocol-version", "2025-06-18")
            .body(Body::from(call.to_string()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["result"]["isError"], false);
}

#[tokio::test]
async fn http_access_key_and_disabled() {
    let app = app("s3cret");
    let ping = json!({"jsonrpc":"2.0","id":1,"method":"ping"});
    let (status, headers, body) = send(
        app.clone(),
        post().body(Body::from(ping.to_string())).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(headers["www-authenticate"], "Bearer");
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["code"], "STORY_ACCESS_DENIED");

    let (status, _, _) = send(
        app.clone(),
        post()
            .header("authorization", "Bearer s3cres")
            .body(Body::from(ping.to_string()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, _) = send(
        app,
        post()
            .header("authorization", "Bearer s3cret")
            .body(Body::from(ping.to_string()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // 既没有密钥也没有公开：谁也进不来。
    let closed = StoryState::with_corpus(corpus(), &Story::default()).unwrap();
    let (router, _) = ik_story::mcp_router().split_for_parts();
    let (status, _, _) = send(
        router.with_state(closed),
        post().body(Body::from(ping.to_string())).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // 还没有语料（数据库里没有生效版本、也没有本地语料）：503。
    let (db, ids) = unused_db();
    let public = Story {
        public: true,
        ..Default::default()
    };
    let disabled = StoryState::new(&public, db, ids).unwrap();
    assert!(disabled.current().is_none());
    let (router, _) = ik_story::mcp_router().split_for_parts();
    let (status, _, body) = send(
        router.with_state(disabled),
        post().body(Body::from(ping.to_string())).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["code"], "STORY_DISABLED");
}

#[tokio::test]
async fn load_local_source_and_reload() {
    let (db, ids) = unused_db();
    let config = Story {
        source: fixture().to_string_lossy().into_owned(),
        ..Default::default()
    };
    // 设置了本地语料：只用它，不碰数据库（这里的数据库连不上）。
    let state = StoryState::new(&config, db.clone(), ids.clone()).unwrap();
    assert!(state.current().is_none());
    state.load().await.unwrap();
    let first = state.current().unwrap();
    assert_eq!(first.version, None);
    assert_eq!(
        first
            .corpus
            .docs
            .iter()
            .filter(|d| d.kind == DocKind::Story)
            .count(),
        4
    );
    assert!(state.spawn_watcher().is_none(), "本地语料不检查数据库版本");
    // 重新加载：原子替换成新的一份，旧的引用仍然可用。
    state.load().await.unwrap();
    let second = state.current().unwrap();
    assert!(!std::sync::Arc::ptr_eq(&first, &second));

    // 语料包文件同样可以作为本地语料。
    let dir = std::env::temp_dir().join(format!("ik-story-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("zzz_story.json.gz");
    std::fs::write(
        &file,
        Bundle::from_dir(&fixture()).unwrap().encode_gzip().unwrap(),
    )
    .unwrap();
    let from_file = ik_story::load_source(&file).unwrap();
    assert_eq!(from_file.text, first.corpus.text);
    std::fs::remove_dir_all(&dir).unwrap();

    let broken = Story {
        source: "/nonexistent/zzz_story.json.gz".to_owned(),
        ..Default::default()
    };
    let state = StoryState::new(&broken, db, ids).unwrap();
    assert!(state.load().await.is_err());
    assert!(state.current().is_none());
}
