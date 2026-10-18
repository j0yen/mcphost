//! PRD-mcphost-elicitation-in-band-on-stateless-transport
//! AC3 (P0) — Given a client that advertises nothing, or one that never
//! echoes `Mcp-Session-Id` (the Claude Agent SDK replay shape), When the tool
//! elicits and nobody answers the link within the wait, Then the `tools/call`
//! response is one `application/json` body carrying
//! `{action:"pending",ask_id,url,expires_unix}` exactly as today. (`melc_ac02`,
//! `melc_ac05`, `melc_ac06`, `melc_ac07` are not touched by this PRD and stay
//! green; the suite runs them unmodified.)

use crate::inband_client;

use crate::common;
use common::{McpClient, TestServer, extract_structured, python_kind_registry, signup};
use inband_client::{RawClient, accept};
use mcphost::sandbox;
use serde_json::{Value, json};
use std::time::Duration;

const TOOL: &str = "import mcphost\ndef main(args):\n    if args.get(\"warm\"):\n        return {\"warm\": True}\n    return mcphost.elicit(\"Rotate?\", {\"confirm\": {\"type\": \"boolean\"}}, timeout_s=1)\n";

#[tokio::test]
async fn no_capability_no_session_echo_and_url_only_all_get_one_json_pending_body() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Elib AC3").await;
    let admin = McpClient::with_bearer(&server.base_url, &key);
    admin
        .tools_call("host.tool_publish", json!({"name": "rotate", "kind": "python", "spec": {"source": TOOL}}))
        .await
        .expect("publish");
    let name = format!("{ns}.rotate");
    // If a frame ever arrives, this client would answer it: the assertions
    // below then fail on `frames`, not hang.
    let answer = |_: &Value| accept(json!({"confirm": true}));

    // (label, capabilities, echoes Mcp-Session-Id)
    let clients = [
        ("advertises nothing", json!({}), true),
        ("form but never echoes the session id", json!({"elicitation": {"form": {}}}), false),
        ("url only", json!({"elicitation": {"url": {}}}), true),
    ];
    let mut asks_seen = 0;
    for (label, capabilities, echo) in clients {
        let client = RawClient::open(&server.base_url, &key, capabilities, echo).await;
        // Warm the tool's environment (the first calls answer `building`).
        for _ in 0..200 {
            let warm = client.call(&name, json!({"warm": true}), &answer).await;
            if warm.response["result"]["structuredContent"]["warm"] == json!(true) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let out = client.call(&name, json!({}), &answer).await;
        assert_eq!(out.content_type, "application/json", "{label}: one JSON body, no SSE");
        assert!(out.frames.is_empty(), "{label}: nothing is sent to the client: {:?}", out.frames);
        let pending = extract_structured(&out.response["result"]);
        assert_eq!(pending["action"], json!("pending"), "{label}: {pending}");
        let ask_id = pending["ask_id"].as_str().expect("ask_id");
        assert!(pending["url"].as_str().is_some_and(|u| u.contains(&format!("/a/{ask_id}"))), "{label}: {pending}");
        assert!(pending["expires_unix"].as_i64().is_some(), "{label}: {pending}");
        asks_seen += 1;
        let rows = extract_structured(&admin.tools_call("host.ask.list", json!({})).await.expect("ask list"));
        let asks = rows["asks"].as_array().expect("asks");
        assert_eq!(asks.len(), asks_seen, "{label}: one link ask per call: {rows}");
        assert!(asks.iter().all(|a| a["channel"] == json!("link")), "{label}: {rows}");
    }
}
