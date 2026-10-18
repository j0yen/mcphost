//! PRD-mcphost-tool-directory-format
//! AC6 (P1) — Given `pack --as-mcp-call`, When the output is POSTed to a
//! fixture host's bare `/mcp`, Then the response carries the published tool's
//! name and an `onboarding.tenant`.
//!
//! The POST is what the PRD's `curl -d @-` sends: the packed body, a
//! `content-type`, an `accept` and the three `MCP-*` headers the stateless
//! host requires of any `tools/call` (named in `docs/tool-directory.md`). The example's tool name is
//! `hello_py` (tool names match `^[a-z][a-z0-9_]{1,40}$`, so `hello-py` is
//! the directory, not the name); `host.tool_publish` answers the qualified
//! name at `name`.

use crate::common;
use common::{TestServer, extract_structured, python_kind_registry};
use serde_json::{Value, json};

#[tokio::test]
async fn tdf_ac06_pack_as_mcp_call_output_publishes_under_implicit_signup() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["tool", "pack", "--as-mcp-call"])
        .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/tools/hello-py"))
        .output()
        .expect("run mcphost tool pack --as-mcp-call");
    assert_eq!(out.status.code(), Some(0));
    let body: Value = serde_json::from_slice(&out.stdout).expect("stdout is one JSON-RPC body");
    assert_eq!(body["method"], json!("tools/call"));
    assert_eq!(body["params"]["name"], json!("host.tool_publish"));

    let data_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&data_dir.0)).await;
    let resp = reqwest::Client::new()
        .post(format!("{}/mcp", server.base_url))
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "tools/call")
        .header("Mcp-Name", "host.tool_publish")
        .body(out.stdout.clone())
        .send()
        .await
        .expect("POST bare /mcp");
    let status = resp.status();
    let text = resp.text().await.unwrap();
    assert!(status.is_success(), "bare /mcp publish: {status} {text}");
    // Implicit signup upgrades the reply to an event stream (the tenant bind
    // emits tools/list_changed first); the result is the last `data:` line.
    let json_text = if text.trim_start().starts_with("data:") {
        text.lines().rev().find_map(|l| l.strip_prefix("data:")).expect("a data line").trim().to_string()
    } else {
        text.clone()
    };
    let reply: Value = serde_json::from_str(&json_text).expect("JSON-RPC response");
    let published = extract_structured(&reply["result"]);

    let name = published["name"].as_str().unwrap_or_else(|| panic!("no name in {published}"));
    assert!(name.ends_with(".hello_py"), "published name: {name}");
    let tenant = published["onboarding"]["tenant"].as_str().expect("onboarding.tenant");
    assert!(name.starts_with(&format!("{tenant}.")), "tool is under the implicit tenant: {name} vs {tenant}");
}
