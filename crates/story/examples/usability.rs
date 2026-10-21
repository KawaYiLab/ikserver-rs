//! Reproducible offline workflows; measures calls and full MCP bytes, not model quality.
use anyhow::{Context, Result, bail};
use ik_story::{corpus::Corpus, mcp::Server};
use serde_json::{Value, json};
use std::{collections::HashSet, time::Instant};

fn rpc(c: &Corpus, s: &Server, tool: &str, args: Value) -> Result<Value> {
    let request = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":tool,"arguments":args}});
    let bytes = s
        .handle(c, &serde_json::to_vec(&request)?)
        .context("missing MCP response")?;
    let response: Value = serde_json::from_slice(&bytes)?;
    if response["result"]["isError"] == true || !response["error"].is_null() {
        bail!("{response}");
    }
    Ok(response)
}
fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let source = args
        .first()
        .context("usage: usability <corpus> <report.json>")?;
    let destination = args.get(1).context("missing report path")?;
    let start = Instant::now();
    let c = ik_story::load_source(std::path::Path::new(source))?;
    let build_ms = start.elapsed().as_secs_f64() * 1000.;
    let server = Server::new()?;
    let definitions = ik_story::mcp::tools::definitions()?;
    let queries = [
        (
            "ik_search",
            json!({"keyword":"天空 核心 秘密","path":"3.3版本"}),
        ),
        (
            "ik_search",
            json!({"keyword":"3.3版本 核心","purpose":"locate"}),
        ),
        (
            "ik_search",
            json!({"keyword":"天空 核心","path":"3.3版本","purpose":"verify","matchUnit":"window","windowLines":12}),
        ),
        ("ik_search", json!({"keyword":"核","path":"3.3版本"})),
        ("ik_catalog", json!({"path":"3.3版本"})),
        ("ik_character", json!({"name":"Fairy"})),
        ("ik_read", json!({"docId":"3.3版本","limit":20})),
    ];
    let mut results = Vec::new();
    for (tool, args) in queries {
        let mut latency = Vec::new();
        let mut output = Value::Null;
        for _ in 0..25 {
            let start = Instant::now();
            output = rpc(&c, &server, tool, args.clone())?;
            latency.push(start.elapsed().as_secs_f64() * 1000.);
        }
        latency.sort_by(f64::total_cmp);
        let full = rpc(&c, &server, tool, {
            let mut a = args.clone();
            a["detail"] = json!("full");
            a
        })?;
        results.push(json!({"tool":tool,"args":args,"p50_ms":latency[12],"p95_ms":latency[23],"text_bytes":output["result"]["content"][0]["text"].as_str().unwrap_or_default().len(),"structured_bytes":serde_json::to_vec(&output["result"]["structuredContent"])?.len(),"rpc_bytes":serde_json::to_vec(&output)?.len(),"full_text_bytes":full["result"]["content"][0]["text"].as_str().unwrap_or_default().len(),"output":output}));
    }
    let mut pages = 0;
    let mut candidates = HashSet::new();
    let mut args = json!({"keyword":"天空 核心 秘密","path":"3.3版本","maxDirectoryResults":50});
    loop {
        let r = rpc(&c, &server, "ik_search", args)?;
        pages += 1;
        let d = &r["result"]["structuredContent"]["data"];
        for item in d["directorySuggestions"]
            .as_array()
            .context("directory results")?
        {
            let id = item["document"]["docId"].as_str().context("docId")?;
            if !candidates.insert(id.to_owned()) {
                bail!("repeated directory doc");
            }
        }
        if d["nextDirectoryCall"].is_null() {
            break;
        }
        args = d["nextDirectoryCall"]["arguments"].clone();
    }
    let catalog = rpc(
        &c,
        &server,
        "ik_catalog",
        json!({"path":"3.3版本","manifest":true}),
    )?;
    let manifest = &catalog["result"]["structuredContent"]["data"]["manifest"];
    let mut args = json!({"docId":"3.3版本","maxChars":30000});
    let mut ledger = Vec::new();
    let (mut calls, mut bytes) = (0, 0);
    loop {
        let r = rpc(&c, &server, "ik_read", args)?;
        calls += 1;
        bytes += serde_json::to_vec(&r)?.len();
        let d = &r["result"]["structuredContent"]["data"];
        ledger.extend(
            d["coverageDelta"]
                .as_array()
                .context("coverageDelta")?
                .iter()
                .cloned(),
        );
        if d["nextCall"].is_null() {
            break;
        }
        args = d["nextCall"]["arguments"].clone();
    }
    let coverage = rpc(
        &c,
        &server,
        "ik_coverage",
        json!({"manifestId":manifest["manifestId"],"ranges":ledger}),
    )?;
    let coverage_data = &coverage["result"]["structuredContent"]["data"];
    if coverage_data["coveredLines"] != manifest["totalLines"]
        || coverage_data["unreadDocuments"] != 0
        || coverage_data["partialDocuments"] != 0
    {
        bail!("chapter coverage mismatch");
    }
    let memory = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!("(Get-Process -Id {}).WorkingSet64", std::process::id()),
        ])
        .output()?;
    let working_set = String::from_utf8_lossy(&memory.stdout)
        .trim()
        .parse::<u64>()?;
    let report = json!({"evaluation":"deterministic MCP workflows; no model-backed quality evaluation","source":source,"corpus_revision":c.revision,"documents":c.docs.len(),"lines":c.lines.len(),"text_bytes":c.text.len(),"build_ms":build_ms,"working_set_bytes":working_set,"tools_list_bytes":serde_json::to_vec(&definitions)?.len(),"queries":results,"directory_workflow":{"calls":pages,"distinct_documents":candidates.len(),"duplicate_documents":0},"chapter_workflow":{"catalog_calls":1,"read_calls":calls,"read_rpc_bytes":bytes,"coverage_calls":1,"data":coverage_data}});
    std::fs::write(destination, serde_json::to_vec_pretty(&report)?)?;
    Ok(())
}
