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

    // 两个词都存在、只是不同篇：部分匹配照常。
    let (text, data) = mcp(&c, "ik_search", json!({"keyword":"录像店 以骸"}));
    assert_eq!(data["partial"], true, "{text}");
    assert!(data["candidateCount"].as_u64().unwrap() >= 2, "{text}");

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
