//! PRD-mcphost-dev-emulator
//! AC2 (P0) -- Given a python tool with `n: integer` in its schema, When
//! `tools/call` sends `{"n": 4.78}`, Then the emulator returns
//! `args_invalid`, message `4.78 is not of type "integer"`,
//! `data.phase = args_coercion`, `data.instance_path = /n`.
//!
//! The message is the one prod returns today (`ArgsError::message`, rendered
//! from jsonschema's own `4.78 is not of type "integer"`): the emulator must
//! not word it differently from the host, so the test pins the host's wording
//! and that jsonschema's text is its source.

use crate::common::ToolDirFixture;
use mcphost::devserver::{Dev, DevOpts, serve};
use serde_json::{Value, json};
use std::sync::Arc;

const REPORT_TOML: &str = "[tool]\nname = \"report\"\nkind = \"python\"\n\n[spec]\nargs_schema = { type = \"object\", properties = { n = { type = \"integer\" } }, required = [\"n\"] }\n";
const REPORT_PY: &str = "def main(args):\n    return {\"n\": args[\"n\"]}\n";

async fn post(addr: std::net::SocketAddr, method: &str, params: Value) -> Value {
    reqwest::Client::new()
        .post(format!("http://{addr}/mcp"))
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}))
        .send()
        .await
        .expect("post /mcp")
        .json()
        .await
        .expect("json body")
}

#[tokio::test]
async fn demu_ac02_fractional_integer_is_args_invalid_with_the_prod_text() {
    let dir = ToolDirFixture::from_example("hello-py");
    dir.write("tool.toml", REPORT_TOML);
    dir.write("tool.py", REPORT_PY);
    let dev = Dev::open(&DevOpts { dir: dir.0.clone(), no_sandbox: true, plan: "free".into() })
        .await
        .expect("open the directory");
    let (addr, task) = serve(Arc::new(dev), 0).await.expect("bind an ephemeral port");

    let listed = post(addr, "tools/list", json!({})).await;
    assert_eq!(listed["result"]["tools"][0]["name"], json!("report"), "{listed}");
    assert_eq!(
        listed["result"]["tools"][0]["inputSchema"]["properties"]["n"]["type"],
        json!("integer"),
        "{listed}"
    );

    let resp = post(addr, "tools/call", json!({"name": "report", "arguments": {"n": 4.78}})).await;
    let data = &resp["error"]["data"];
    assert_eq!(data["error_code"], json!("args_invalid"), "{resp}");
    let schema = json!({"type": "object", "properties": {"n": {"type": "integer"}}});
    let instance = json!({"n": 4.78});
    let validator = jsonschema::validator_for(&schema).expect("schema");
    let host_err = validator.validate(&instance).expect_err("4.78 is not an integer");
    assert_eq!(host_err.to_string(), "4.78 is not of type \"integer\"");
    let host_text = mcphost::kinds::describe_args_error(&host_err).message();
    assert_eq!(resp["error"]["message"], json!(host_text), "the host's own text: {resp}");
    assert!(host_text.contains("expected integer, got number (4.78)"), "{host_text}");
    assert_eq!(data["phase"], json!("args_coercion"), "{resp}");
    assert_eq!(data["instance_path"], json!("/n"), "{resp}");
    assert_eq!(resp["emulator"]["sandbox"], json!("none"), "{resp}");

    let ok = post(addr, "tools/call", json!({"name": "report", "arguments": {"n": 4}})).await;
    assert_eq!(ok["result"]["structuredContent"]["n"], json!(4), "{ok}");
    task.abort();
}
