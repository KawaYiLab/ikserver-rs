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
    let a = ok(&c, "zzz_search", json!({ "keyword": "狡兔屋" }));
    let b = ok(&rebuilt, "zzz_search", json!({ "keyword": "狡兔屋" }));
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
    let out = ok(&c, "zzz_search", json!({ "keyword": "狡兔屋 妮可" }));
    assert!(out.contains("命中"), "{out}");
    assert!(!out.contains("部分匹配"));
    assert!(
        out.contains("妮可：安比，交给你了！狡兔屋的招牌可不能砸。"),
        "{out}"
    );

    // 说话人过滤：只保留妮可本人的台词。
    let out = ok(
        &c,
        "zzz_search",
        json!({ "keyword": "狡兔屋", "speaker": "妮可" }),
    );
    assert!(out.contains("妮可：安比"), "{out}");
    assert!(!out.contains("哲：绳网上"), "{out}");

    // 范围：设定。
    let out = ok(
        &c,
        "zzz_search",
        json!({ "keyword": "赤字", "scope": "lore" }),
    );
    assert!(out.contains("妮可（妮可·德玛拉）"), "{out}");
    assert!(!out.contains("主线 ›"), "{out}");

    // 没有共同出现时退化为部分匹配。
    let out = ok(&c, "zzz_search", json!({ "keyword": "猫又 赫利俄斯" }));
    assert!(out.contains("部分匹配"), "{out}");

    // 全角、大小写折叠。
    let out = ok(&c, "zzz_search", json!({ "keyword": "ＦＡＩＲＹ" }));
    assert!(out.contains("Fairy：检测到以太活性异常升高。"), "{out}");

    // 只按说话人列出台词。
    let out = ok(&c, "zzz_search", json!({ "speaker": "零号·安比" }));
    assert!(out.contains("零号·安比：这里交给我。"), "{out}");

    let err = call(
        &c,
        "zzz_search",
        json!({ "keyword": "x", "speaker": "不存在的人" }),
    )
    .unwrap_err();
    assert!(err.contains("没有名为"), "{err}");
    let err = call(
        &c,
        "zzz_search",
        json!({ "keyword": "x", "scope": "bogus" }),
    )
    .unwrap_err();
    assert!(err.contains("scope"), "{err}");
    let out = ok(&c, "zzz_search", json!({ "keyword": "完全不存在的词组" }));
    assert!(out.contains("没有命中"), "{out}");
}

#[test]
fn search_within_doc_and_path() {
    let c = corpus();
    let found = ok(&c, "zzz_search", json!({ "keyword": "赤字" }));
    let doc = first_doc_id(&found);
    let out = ok(
        &c,
        "zzz_search",
        json!({ "docId": doc, "keyword": "狡兔屋" }),
    );
    assert!(out.starts_with("[zzz_search] 篇内检索"), "{out}");
    assert!(out.contains("命中 2 行"), "{out}");

    let out = ok(
        &c,
        "zzz_search",
        json!({ "keyword": "零号", "path": "第一季 第一章" }),
    );
    assert!(out.contains("零号空洞"), "{out}");
    let out = ok(
        &c,
        "zzz_search",
        json!({ "keyword": "妮可", "path": "Chapter01" }),
    );
    assert!(out.contains("没有命中"), "{out}");
}

#[test]
fn read_pages_and_follows_across_docs() {
    let c = corpus();
    // 分组 id 从章首开始并默认 follow，跨场景读到章末。
    let out = ok(
        &c,
        "zzz_read",
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
    assert!(out.contains("已全部读完"), "{out}");
    assert!(out.contains("下一组：ch:Chapter01"), "{out}");

    // 行数上限 → 给出精确续读游标。
    let out = ok(
        &c,
        "zzz_read",
        json!({ "docId": "ch:Chapter00", "limit": 2 }),
    );
    assert!(out.contains("第 1-2 行"), "{out}");
    assert!(out.contains("offset=3, follow=true"), "{out}");

    // 单篇读完给出下一篇。
    let first = first_doc_id(&out);
    let out = ok(&c, "zzz_read", json!({ "docId": first, "offset": 6 }));
    assert!(out.contains("本篇完。下一篇"), "{out}");
    assert!(out.contains("L6 妮可：法厄同"), "{out}");

    let out = ok(
        &c,
        "zzz_read",
        json!({ "docId": first, "lineNumbers": "false", "limit": "1" }),
    );
    assert!(out.contains("\n六分街"), "{out}");

    let err = call(&c, "zzz_read", json!({ "docId": first, "offset": 99 })).unwrap_err();
    assert!(err.contains("超出范围"), "{err}");
    let err = call(&c, "zzz_read", json!({ "docId": "doc_000000000000" })).unwrap_err();
    assert!(err.contains("找不到文档"), "{err}");
    let err = call(&c, "zzz_read", json!({})).unwrap_err();
    assert!(err.contains("docId"), "{err}");
}

#[test]
fn catalog_root_group_and_doc() {
    let c = corpus();
    let root = ok(&c, "zzz_catalog", json!({}));
    assert!(root.contains("ch:Chapter00 序章 | 3 场景"), "{root}");
    assert!(root.contains("lore:agents 代理人档案 | 2 篇"), "{root}");
    assert!(root.contains("说话人覆盖 79%"), "{root}");

    let group = ok(&c, "zzz_catalog", json!({ "path": "序章" }));
    assert!(group.contains("#### 任务：开张大吉"), "{group}");
    assert!(
        group.contains("录像店的早晨 · 6句 · 铃、哲、妮可"),
        "{group}"
    );
    assert!(group.contains("### 补充 · 过场字幕"), "{group}");

    let filtered = ok(&c, "zzz_catalog", json!({ "keyword": "妮可" }));
    assert!(filtered.contains("妮可（妮可·德玛拉）"), "{filtered}");
    assert!(filtered.contains("主线聊天·妮可"), "{filtered}");

    let doc = first_doc_id(&group);
    let card = ok(&c, "zzz_catalog", json!({ "path": doc }));
    assert!(
        card.contains("所属：ch:Chapter00 序章（第 1/4 篇）"),
        "{card}"
    );
    assert!(card.contains("下一篇："), "{card}");

    let paged = ok(
        &c,
        "zzz_catalog",
        json!({ "path": "ch:Chapter00", "maxResults": 1 }),
    );
    assert!(
        paged.contains("还有 3 篇：zzz_catalog(path=\"ch:Chapter00\", offset=1)"),
        "{paged}"
    );

    let err = call(&c, "zzz_catalog", json!({ "path": "不存在的章节" })).unwrap_err();
    assert!(err.contains("找不到分组"), "{err}");
}

#[test]
fn character_profile() {
    let c = corpus();
    let out = ok(&c, "zzz_character", json!({ "name": "安比" }));
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

    let out = ok(&c, "zzz_character", json!({ "name": "赫利俄斯" }));
    assert!(out.contains("不是已知说话人"), "{out}");
    let err = call(&c, "zzz_character", json!({ "name": "查无此人" })).unwrap_err();
    assert!(err.contains("没有找到"), "{err}");
}

#[test]
fn guide_modes() {
    let c = corpus();
    assert!(ok(&c, "zzz_guide", json!({ "mode": "summary" })).contains("分层压缩"));
    assert!(ok(&c, "zzz_guide", json!({ "mode": "查证" })).contains("原文查证"));
    assert!(
        call(&c, "zzz_guide", json!({ "mode": "x" }))
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
            .contains("zzz_search")
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
            "zzz_catalog",
            "zzz_search",
            "zzz_read",
            "zzz_character",
            "zzz_guide"
        ]
    );
    for t in list["result"]["tools"].as_array().unwrap() {
        assert_eq!(t["inputSchema"]["type"], "object");
        assert_eq!(t["annotations"]["readOnlyHint"], true);
    }

    let res = rpc(
        &s,
        json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"zzz_search","arguments":{"keyword":"狡兔屋"}}}),
    );
    assert_eq!(res["result"]["isError"], false);
    assert!(
        res["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .starts_with("[zzz_search]")
    );

    // 工具执行错误 → isError，而不是协议错误。
    let res = rpc(
        &s,
        json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"zzz_read","arguments":{"docId":"doc_bad"}}}),
    );
    assert_eq!(res["result"]["isError"], true);
    let res = rpc(
        &s,
        json!({"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"zzz_read","arguments":{"offset":"abc","docId":"x"}}}),
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

    let call = json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"zzz_search","arguments":{"keyword":"狡兔屋"}}});
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
