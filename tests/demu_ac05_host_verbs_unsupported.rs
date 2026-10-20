//! PRD-mcphost-dev-emulator
//! AC5 (P0) -- Given `host.trigger.set` sent to the emulator, When
//! dispatched, Then the response is `emulator_unsupported` naming the verb
//! and the hint `publish with mcphost push, then host.trigger.set on the
//! host`.

use crate::common::ToolDirFixture;
use mcphost::devserver::{Dev, DevOpts, serve};
use serde_json::{Value, json};
use std::sync::Arc;

const TOML: &str = "[tool]\nname = \"report\"\nkind = \"python\"\n\n[spec]\nargs_schema = { type = \"object\" }\n";
const PY: &str = "def main(args):\n    return {\"ran\": True}\n";

async fn call(addr: std::net::SocketAddr, name: &str, arguments: Value) -> Value {
    reqwest::Client::new()
        .post(format!("http://{addr}/mcp"))
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": arguments}}))
        .send()
        .await
        .expect("post /mcp")
        .json()
        .await
        .expect("json body")
}

#[tokio::test]
async fn demu_ac05_trigger_set_is_emulator_unsupported_with_the_publish_hint() {
    let dir = ToolDirFixture::from_example("hello-py");
    dir.write("tool.toml", TOML);
    dir.write("tool.py", PY);
    let dev = Dev::open(&DevOpts { dir: dir.0.clone(), no_sandbox: true, plan: "free".into() })
        .await
        .expect("open the directory");
    let (addr, task) = serve(Arc::new(dev), 0).await.expect("bind");

    let resp = call(addr, "host.trigger.set", json!({"tool": "report", "cron": "0 * * * *"})).await;
    let data = &resp["error"]["data"];
    assert_eq!(data["error_code"], json!("emulator_unsupported"), "{resp}");
    assert_eq!(data["verb"], json!("host.trigger.set"), "{resp}");
    assert_eq!(data["hint"], json!("publish with mcphost push, then host.trigger.set on the host"), "{resp}");
    assert!(resp["error"]["message"].as_str().unwrap_or_default().contains("host.trigger.set"), "{resp}");

    // The verbs the emulator does serve are untouched, and an unknown tool is still tool_not_found.
    let ran = call(addr, "host.tool_test", json!({"name": "report", "args": {}})).await;
    assert_eq!(ran["result"]["structuredContent"]["result"]["ran"], json!(true), "{ran}");
    let missing = call(addr, "no_such_tool", json!({})).await;
    assert_eq!(missing["error"]["data"]["error_code"], json!("tool_not_found"), "{missing}");
    task.abort();
}
