//! PRD-mcphost-elicitation-in-band-on-stateless-transport
//! AC1 (P0) — Given `TestServer` and a client declaring
//! `{"elicitation":{"form":{}}}` that echoes `Mcp-Session-Id` and answers
//! `accept {confirm:true}`, When a tool calls `mcphost.elicit` in
//! `tools/call`, Then the client gets `elicitation/create` on the call's own
//! stream and the tool gets the accepted `content` in < 2 s measured from the
//! first non-`tool_building` `tools/call`, one `channel: client` audit row, no
//! `url` row.
//!
//! RED at 0fdd8e53: prod answered `pending` and no server→client request ever
//! reached the client. `TestServer` serves `build_router`, the production
//! constructor (AC5 pins that).

use crate::inband_client;

use crate::common;
use common::{McpClient, TestServer, extract_structured, python_kind_registry, signup};
use inband_client::{RawClient, accept};
use mcphost::sandbox;
use serde_json::{Value, json};
use std::time::Duration;

const TOOL: &str = "import mcphost\ndef main(args):\n    return mcphost.elicit(\"Rotate?\", {\"confirm\": {\"type\": \"boolean\"}})\n";

fn building(resp: &Value) -> bool {
    resp["result"].get("structuredContent").is_some_and(|s| s["status"] == json!("building"))
        || resp["error"]["data"]["error_code"] == json!("tool_building")
}

#[tokio::test]
async fn form_client_gets_elicitation_create_on_the_calls_own_stream_and_the_tool_gets_the_content() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Elib AC1").await;
    let admin = McpClient::with_bearer(&server.base_url, &key);
    admin
        .tools_call("host.tool_publish", json!({"name": "rotate", "kind": "python", "spec": {"source": TOOL}}))
        .await
        .expect("publish");

    let client = RawClient::open(&server.base_url, &key, json!({"elicitation": {"form": {}}}), true).await;
    let name = format!("{ns}.rotate");
    let answer = |_: &Value| accept(json!({"confirm": true}));
    // The first call that is not `tool_building` is the one measured.
    let mut outcome = client.call(&name, json!({}), &answer).await;
    for _ in 0..200 {
        if !building(&outcome.response) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        outcome = client.call(&name, json!({}), &answer).await;
    }
    assert!(!building(&outcome.response), "tool never became ready");

    assert_eq!(outcome.content_type, "text/event-stream", "an eliciting call streams");
    assert_eq!(outcome.frames.len(), 1, "{:?}", outcome.frames);
    let frame = &outcome.frames[0];
    assert_eq!(frame["method"], json!("elicitation/create"), "{frame}");
    assert_eq!(frame["params"]["message"], json!("Rotate?"), "{frame}");
    assert_eq!(frame["params"]["requestedSchema"]["properties"]["confirm"]["type"], json!("boolean"), "{frame}");
    assert_eq!(outcome.answer_statuses, vec![202], "the answer POST is acknowledged with 202");
    assert_eq!(
        extract_structured(&outcome.response["result"]),
        json!({"action": "accept", "content": {"confirm": true}}),
        "{}",
        outcome.response
    );

    // One audit row, on the client channel; no link (`url`) ask was opened.
    let rows = extract_structured(&admin.tools_call("host.ask.list", json!({})).await.expect("ask list"));
    let asks = rows["asks"].as_array().expect("asks");
    assert_eq!(asks.len(), 1, "{rows}");
    assert_eq!(asks[0]["channel"], json!("client"), "{rows}");
    assert_eq!(asks[0]["question"], json!("Rotate?"), "{rows}");
    assert!(asks.iter().all(|a| a["channel"] != json!("url")), "{rows}");

    // Measured from a ready call: well under the 25 s link wait.
    crate::perf_budget!(2000, {
        let again = client.call(&name, json!({}), &answer).await;
        assert_eq!(extract_structured(&again.response["result"])["action"], json!("accept"));
    });
}
