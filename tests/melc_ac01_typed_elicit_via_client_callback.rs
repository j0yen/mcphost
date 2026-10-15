//! PRD-mcphost-elicitation-with-ask-fallback
//! AC1 (P0) — Given mcphost built with rmcp's `elicitation` feature and a
//! client whose elicitation callback answers `{confirm: true}`, When a python
//! tool calls `mcphost.elicit("Rotate?", {confirm: {type: "boolean"}})` inside
//! `tools/call`, Then the tool receives `{action: "accept", content:
//! {confirm: true}}` within one call and the ask-user audit has one row with
//! `channel: client`.
//!
//! The fixture is `support/elicit_session.rs`: a session-mode client playing
//! python-mcp's `elicitation_callback` (python-mcp itself is not installed in
//! the gate; the wire exchange is the same `elicitation/create` request).

use crate::common;
use crate::elicit_session;
use common::{McpClient, TestServer, extract_structured, python_kind_registry, signup};
use elicit_session::{Reply, SessionClient};
use mcphost::sandbox;
use serde_json::{Value, json};
use std::time::Duration;

const TOOL: &str = "import mcphost\ndef main(args):\n    return mcphost.elicit(\"Rotate?\", {\"confirm\": {\"type\": \"boolean\"}})\n";

/// Calls until the tool's environment is built.
async fn call_ready(client: &SessionClient, name: &str, reply: &Reply) -> Value {
    for _ in 0..200 {
        let resp = client.call(name, json!({}), reply).await;
        let building = resp["result"]
            .get("structuredContent")
            .is_some_and(|s| s["status"] == json!("building"))
            || resp["error"]["data"]["error_code"] == json!("tool_building");
        if !building {
            return resp;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("tool never became ready");
}

#[tokio::test]
async fn python_elicit_gets_the_client_form_answer_and_one_client_audit_row() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Melc AC1").await;
    let admin = McpClient::with_bearer(&server.base_url, &key);
    admin
        .tools_call("host.tool_publish", json!({"name": "rotate", "kind": "python", "spec": {"source": TOOL}}))
        .await
        .expect("publish");

    let session_base = elicit_session::serve_session_mode(server.state.clone()).await;
    let client = SessionClient::open(&session_base, &key, true).await;
    let resp = call_ready(&client, &format!("{ns}.rotate"), &Reply::Accept(json!({"confirm": true}))).await;
    let out = extract_structured(&resp["result"]);
    assert_eq!(out, json!({"action": "accept", "content": {"confirm": true}}), "{resp}");
    assert_eq!(client.received.lock().unwrap().len(), 1, "exactly one elicitation/create reached the client");

    let rows = extract_structured(&admin.tools_call("host.ask.list", json!({})).await.expect("ask list"));
    let asks = rows["asks"].as_array().expect("asks");
    assert_eq!(asks.len(), 1, "{rows}");
    assert_eq!(asks[0]["channel"], json!("client"), "{rows}");
    assert_eq!(asks[0]["question"], json!("Rotate?"), "{rows}");
}
