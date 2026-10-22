//! 会误导模型下判断的输出语义：缺失词的作用域、零命中词不退化为并集、批量读取未读完的篇目。
//! 走 MCP 默认（compact）路径，断言模型实际看到的文本与 structuredContent。

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

use serde_json::{Value, json};

use ik_story::corpus::{Corpus, build_from_dir};
use ik_story::mcp::tools;

fn corpus() -> Corpus {
    build_from_dir(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample"))
        .unwrap()
}

/// MCP 调用：返回（文本，structuredContent.data）。
fn mcp(c: &Corpus, tool: &str, args: Value) -> (String, Value) {
    match tools::call_output(c, tool, Some(args)) {
        Ok(Ok(out)) => {
            let response = serde_json::to_value(&out.response).unwrap();
            (out.text, response["data"].clone())
        }
        Ok(Err(e)) => panic!("{tool} failed: {}", e.0),
        Err(_) => panic!("unknown tool {tool}"),
    }
}

fn suggestion_keywords(data: &Value) -> Vec<(String, Option<String>)> {
    data["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            let a = &s["arguments"];
            (
                a["keyword"].as_str().unwrap_or("").to_owned(),
                a["speaker"].as_str().map(str::to_owned),
            )
        })
        .collect()
}

#[test]
fn missing_term_in_filtered_scope_is_not_reported_as_missing_from_the_corpus() {
    let c = corpus();
    // 「铃」在全库存在，但安比的台词里没有：不能说成「全局缺失」。
    let (text, data) = mcp(&c, "ik_search", json!({"keyword":"铃","speaker":"安比"}));
    assert!(!text.contains("全局"), "{text}");
    assert!(text.contains("当前范围内无命中：铃（全库 "), "{text}");
    // 给出的去掉 speaker 的调用会扩大范围，文案必须如实说明。
    assert!(text.contains("会扩大检索范围"), "{text}");
    assert!(
        suggestion_keywords(&data).contains(&("铃".into(), None)),
        "{data}"
    );
    assert_eq!(data["missingTerms"], json!(["铃"]));
    assert!(
        data["missingTermCounts"][0]["corpusDocuments"]
            .as_u64()
            .unwrap()
            > 0
    );
    // 不再建议与刚才完全相同的调用（模型会原地重试）。
    assert!(
        !suggestion_keywords(&data).contains(&("铃".into(), Some("安比".into()))),
        "{data}"
    );

    // 全库都没有的词明确说「全库无此词」。
    let (text, data) = mcp(&c, "ik_search", json!({"keyword":"旧账"}));
    assert!(text.contains("全库无此词：旧账"), "{text}");
    assert!(!text.contains("全局"), "{text}");
    assert_eq!(data["missingTermCounts"][0]["corpusDocuments"], 0);
}

#[test]
fn a_term_with_no_hits_does_not_degrade_the_query_into_the_other_terms_union() {
    let c = corpus();
    // 「旧账」零命中：并集只会等于单搜「安比」，不能当作「部分匹配」铺满结果。
    let (text, data) = mcp(&c, "ik_search", json!({"keyword":"安比 旧账"}));
    assert_eq!(data["partial"], false, "{text}");
    assert_eq!(data["candidateCount"], 0, "{text}");
    assert!(!text.contains("未命中关键词"), "{text}");
    assert!(text.contains("未退化为部分匹配"), "{text}");
    assert!(
        suggestion_keywords(&data).contains(&("安比".into(), None)),
        "{data}"
    );

    // 两个词都存在、但没有任何一篇同时出现：合并只是各词单独结果的拼接，不含共现信息。
    // 默认不合并，页眉一次报各词篇数，并按词分别给出检索调用。
    let (text, data) = mcp(&c, "ik_search", json!({"keyword":"录像店 以骸"}));
    assert_eq!(data["partial"], false, "{text}");
    assert_eq!(data["candidateCount"], 0, "{text}");
    assert!(text.contains("未退化为部分匹配"), "{text}");
    assert!(
        text.contains("录像店 1 篇") && text.contains("以骸 1 篇"),
        "{text}"
    );
    assert!(!text.contains("未命中关键词"), "{text}");
    assert_eq!(data["partialWithheld"], true);
    assert!(!text.contains("没有命中。"), "{text}");
    assert_eq!(
        data["termDocuments"],
        json!([{"term":"录像店","documents":1},{"term":"以骸","documents":1}])
    );
    let keywords = suggestion_keywords(&data);
    assert!(keywords.contains(&("录像店".into(), None)), "{data}");
    assert!(keywords.contains(&("以骸".into(), None)), "{data}");

    // 所有词都零命中：没有可合并的东西，按普通「没有命中」处理。
    let (text, data) = mcp(&c, "ik_search", json!({"keyword":"旧账 不存在的词"}));
    assert!(data.get("partialWithheld").is_none(), "{data}");
    assert!(
        text.contains("没有命中") && !text.contains("未退化"),
        "{text}"
    );

    // 显式 allowPartial=true 仍按要求合并。
    let (text, data) = mcp(
        &c,
        "ik_search",
        json!({"keyword":"录像店 以骸","allowPartial":true}),
    );
    assert_eq!(data["partial"], true, "{text}");
    assert_eq!(data["candidateCount"], 2, "{text}");

    // 有两词同篇（2/3）时，部分匹配才有信息量：照常合并，页眉一次说明最多命中几个词。
    let (text, data) = mcp(&c, "ik_search", json!({"keyword":"以骸 狡兔屋 录像店"}));
    assert_eq!(data["partial"], true, "{text}");
    assert!(text.contains("最多命中 2/3 词"), "{text}");
    assert!(text.contains("以骸 1 篇"), "{text}");

    // 一个词零命中、其余两词同篇：建议直接检索其余两词的完整匹配，一跳到位。
    let (text, data) = mcp(&c, "ik_search", json!({"keyword":"以骸 狡兔屋 旧账"}));
    assert_eq!(data["candidateCount"], 0, "{text}");
    assert!(
        suggestion_keywords(&data).contains(&("以骸 狡兔屋".into(), None)),
        "{data}"
    );

    // allowPartial=false 的严格语义不变。
    let (_, data) = mcp(
        &c,
        "ik_search",
        json!({"keyword":"录像店 以骸","allowPartial":false}),
    );
    assert_eq!(data["candidateCount"], 0);
}

#[test]
fn batch_read_reports_documents_it_did_not_reach() {
    let c = corpus();
    let group = c.groups.iter().find(|g| g.id == "ch:Chapter00").unwrap();
    let first = c.doc_id(group.docs[0] as usize);
    let second = c.doc_id(group.docs[1] as usize);
    let first_lines = c.docs[group.docs[0] as usize].line_count;

    // 第一篇没读完：第二篇一行都没读，必须点名。
    let (text, data) = mcp(&c, "ik_read", json!({"docIds":[first, second],"limit":2}));
    assert!(text.contains("剩余 1 篇未开始"), "{text}");
    assert!(text.contains(&second), "{text}");
    assert_eq!(data["pendingDocuments"], json!([second]));

    // 第一篇恰好读完、预算用尽：同样点名第二篇。
    let (text, data) = mcp(
        &c,
        "ik_read",
        json!({"docIds":[first, second],"limit":first_lines}),
    );
    assert!(text.contains("剩余 1 篇未开始"), "{text}");
    assert_eq!(data["pendingDocuments"], json!([second]));

    // 全部读完：没有未读提示。
    let (text, data) = mcp(&c, "ik_read", json!({"docIds":[first, second]}));
    assert!(!text.contains("未开始"), "{text}");
    assert!(data.get("pendingDocuments").is_none(), "{data}");

    // 单篇读取路径不受影响。
    let (text, _) = mcp(&c, "ik_read", json!({"docId":first,"limit":2}));
    assert!(!text.contains("未开始"), "{text}");
}

#[test]
fn full_search_does_not_print_an_empty_body_missing_label() {
    let c = corpus();
    let (text, _) = mcp(&c, "ik_search", json!({"keyword":"安比","detail":"full"}));
    assert!(
        !text.lines().any(|l| l.trim_end() == "正文缺失词："),
        "{text}"
    );
    // 真有缺失时照常列出。
    let (text, _) = mcp(
        &c,
        "ik_search",
        json!({"keyword":"录像店 以骸","detail":"full","allowPartial":true}),
    );
    assert!(text.contains("正文缺失词：以骸"), "{text}");
}
