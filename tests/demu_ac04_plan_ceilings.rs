//! PRD-mcphost-dev-emulator
//! AC4 (P0) -- Given `--plan free` and a call with
//! `budget.max_tool_latency_ms = 180000`, When the call is made, Then the
//! emulator returns the same text prod returns (`exceeds this plan's ceiling
//! of 30000`); Given `--plan pro`, Then the call runs.
//!
//! Prod answers a budget over the ceiling with `budget_ceiling_exceeded`
//! (`budget::resolve_and_validate`, a JSON-RPC invalid-params error like
//! `args_invalid`); the emulator calls the same function, so the code and
//! text are the host's. `pro`'s own ceiling is 120000, so the pro half sends
//! a value between the two plans' ceilings, and 180000 is refused there too.

use crate::common::ToolDirFixture;
use mcphost::devserver::{Dev, DevOpts, serve};
use mcphost::plans::PlanCatalog;
use serde_json::{Value, json};
use std::sync::Arc;

const TOML: &str = "[tool]\nname = \"report\"\nkind = \"python\"\n\n[spec]\nargs_schema = { type = \"object\" }\n";
const PY: &str = "def main(args):\n    return {\"ran\": True}\n";

async fn call(addr: std::net::SocketAddr, latency_ms: i64) -> Value {
    reqwest::Client::new()
        .post(format!("http://{addr}/mcp"))
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
            "name": "host.tool_call",
            "arguments": {"name": "report", "args": {}, "budget": {"max_tool_latency_ms": latency_ms}},
        }}))
        .send()
        .await
        .expect("post /mcp")
        .json()
        .await
        .expect("json body")
}

async fn start(plan: &str) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>, ToolDirFixture) {
    let dir = ToolDirFixture::from_example("hello-py");
    dir.write("tool.toml", TOML);
    dir.write("tool.py", PY);
    let dev = Dev::open(&DevOpts { dir: dir.0.clone(), no_sandbox: true, plan: plan.into() })
        .await
        .expect("open the directory");
    let (addr, task) = serve(Arc::new(dev), 0).await.expect("bind");
    (addr, task, dir)
}

#[tokio::test]
async fn demu_ac04_free_refuses_and_pro_runs_from_the_same_plan_table() {
    let catalog = PlanCatalog::default_catalog();
    let free = catalog.get("free").expect("free plan").budget_max_tool_latency_ms;
    let pro = catalog.get("pro").expect("pro plan").budget_max_tool_latency_ms;
    assert_eq!((free, pro), (30_000, 120_000), "the plan table the emulator reads");

    let (addr, task, _dir) = start("free").await;
    let refused = call(addr, 180_000).await;
    assert_eq!(refused["error"]["data"]["error_code"], json!("budget_ceiling_exceeded"), "{refused}");
    assert_eq!(
        refused["error"]["message"],
        json!("budget.max_tool_latency_ms: 180000 exceeds this plan's ceiling of 30000"),
        "{refused}"
    );
    assert_eq!(refused["error"]["data"]["ceiling"], json!(30_000), "{refused}");
    assert_eq!(refused["emulator"]["plan"], json!("free"), "{refused}");
    let within = call(addr, 30_000).await;
    assert_eq!(within["result"]["structuredContent"]["ran"], json!(true), "{within}");
    task.abort();

    // `network: public` is pro-only at publish; the emulator refuses it at start.
    let gated = ToolDirFixture::from_example("hello-py");
    gated.write("tool.toml", &TOML.replace("args_schema", "network = \"public\"\nargs_schema"));
    gated.write("tool.py", PY);
    let opts = |plan: &str| DevOpts { dir: gated.0.clone(), no_sandbox: true, plan: plan.into() };
    let refusal = Dev::open(&opts("free")).await.err().expect("free refuses network: public");
    assert!(refusal.message.contains("requires the pro plan"), "{refusal:?}");
    Dev::open(&opts("pro")).await.expect("pro lifts it");

    let (addr, task, _dir) = start("pro").await;
    let ran = call(addr, 100_000).await;
    assert_eq!(ran["result"]["structuredContent"]["ran"], json!(true), "{ran}");
    assert_eq!(ran["emulator"]["plan"], json!("pro"), "{ran}");
    let over = call(addr, 180_000).await;
    assert!(
        over["error"]["message"].as_str().unwrap_or_default().contains("exceeds this plan's ceiling of 120000"),
        "{over}"
    );
    task.abort();
}
