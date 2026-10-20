//! PRD-mcphost-dev-emulator
//! AC3 (P0) -- Given a box where `sandbox-check` finds bubblewrap, When
//! `mcphost dev` runs without `--no-sandbox` and the tool does `import
//! socket; socket.create_connection(("1.1.1.1", 53))`, Then the call fails
//! inside the sandbox and the response carries `emulator.sandbox = "bwrap"`;
//! Given `--no-sandbox`, Then every response carries `emulator.sandbox =
//! "none"`.

use crate::common::ToolDirFixture;
use mcphost::devserver::{Dev, DevOpts, serve};
use mcphost::sandbox::{self, IsolationMechanism};
use serde_json::{Value, json};
use std::sync::Arc;

const PROBE_TOML: &str = "[tool]\nname = \"probe\"\nkind = \"python\"\n\n[spec]\nargs_schema = { type = \"object\" }\n";
const PROBE_PY: &str = "import socket\n\n\ndef main(args):\n    socket.create_connection((\"1.1.1.1\", 53), timeout=3).close()\n    return {\"connected\": True}\n";
const ECHO_PY: &str = "def main(args):\n    return {\"ok\": True}\n";

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

async fn start(source: &str, no_sandbox: bool) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>, ToolDirFixture) {
    let dir = ToolDirFixture::from_example("hello-py");
    dir.write("tool.toml", PROBE_TOML);
    dir.write("tool.py", source);
    let dev = Dev::open(&DevOpts { dir: dir.0.clone(), no_sandbox, plan: "free".into() })
        .await
        .expect("open the directory");
    let (addr, task) = serve(Arc::new(dev), 0).await.expect("bind");
    (addr, task, dir)
}

#[tokio::test]
async fn demu_ac03_sandbox_marker_is_bwrap_or_none_on_every_response() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }

    // --no-sandbox: every response, of every method, says "none".
    let (addr, task, _dir) = start(ECHO_PY, true).await;
    for (method, params) in [
        ("initialize", json!({})),
        ("tools/list", json!({})),
        ("tools/call", json!({"name": "probe", "arguments": {}})),
        ("tools/call", json!({"name": "no_such_tool", "arguments": {}})),
    ] {
        let resp = post(addr, method, params).await;
        assert_eq!(resp["emulator"]["sandbox"], json!("none"), "{method}: {resp}");
    }
    task.abort();

    if sandbox::detect_mechanism() != IsolationMechanism::Bwrap {
        println!("bubblewrap not found by sandbox-check: bwrap half skipped");
        return;
    }
    // Sandboxed: the connect fails inside bwrap, and the stamp says so.
    let (addr, task, _dir) = start(PROBE_PY, false).await;
    let resp = post(addr, "tools/call", json!({"name": "probe", "arguments": {}})).await;
    assert_eq!(resp["emulator"]["sandbox"], json!("bwrap"), "{resp}");
    assert_eq!(resp["error"]["data"]["error_code"], json!("tool_exception"), "{resp}");
    assert_eq!(resp["error"]["data"]["exception_class"], json!("OSError"), "the connect failed in the sandbox: {resp}");
    assert!(resp.get("result").is_none(), "no connection was made: {resp}");
    let listed = post(addr, "tools/list", json!({})).await;
    assert_eq!(listed["emulator"]["sandbox"], json!("bwrap"), "{listed}");
    task.abort();
}
