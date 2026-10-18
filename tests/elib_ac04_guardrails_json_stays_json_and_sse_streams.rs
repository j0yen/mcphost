//! PRD-mcphost-elicitation-in-band-on-stateless-transport
//! AC4 (P0) — Given the same binary, When `compat_ac11_ac12_claude_sdk_replay`
//! and `sessbind_ac01`..`ac11` run, Then they pass unchanged (the suite runs
//! them unmodified); a non-eliciting `tools/call` from an
//! elicitation-advertising session is still one `application/json` body (no
//! SSE) with the same status and headers as before; and a `text/event-stream`
//! body served through `build_router` on `/mcp` delivers its first event
//! before the stream closes (R0: `oauth_401_upgrade` buffers only
//! `application/json`).

use crate::inband_client;

use crate::common;
use common::{McpClient, TestServer, extract_structured, python_kind_registry, signup};
use inband_client::{RawClient, silent};
use mcphost::sandbox;
use serde_json::{Value, json};
use std::time::Duration;

const TOOL: &str = "import mcphost\ndef main(args):\n    if args.get(\"warm\"):\n        return {\"warm\": True}\n    return mcphost.elicit(\"Rotate?\", {\"confirm\": {\"type\": \"boolean\"}}, timeout_s=3)\n";

async fn warm(client: &RawClient, name: &str) {
    for _ in 0..200 {
        let out = client.call(name, json!({"warm": true}), &silent).await;
        if out.response["result"]["structuredContent"]["warm"] == json!(true) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("tool never became ready");
}

#[tokio::test]
async fn non_eliciting_call_from_an_advertising_session_is_unchanged_json() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Elib AC4 json").await;
    let admin = McpClient::with_bearer(&server.base_url, &key);
    admin
        .tools_call("host.tool_publish", json!({"name": "rotate", "kind": "python", "spec": {"source": TOOL}}))
        .await
        .expect("publish");
    let name = format!("{ns}.rotate");
    let advertising = RawClient::open(&server.base_url, &key, json!({"elicitation": {"form": {}}}), true).await;
    let plain = RawClient::open(&server.base_url, &key, json!({}), false).await;
    warm(&advertising, &name).await;
    warm(&plain, &name).await;

    let a = advertising.call(&name, json!({"warm": true}), &silent).await;
    let b = plain.call(&name, json!({"warm": true}), &silent).await;
    assert_eq!(a.content_type, "application/json", "no SSE for a call that never elicits");
    assert_eq!(a.status, b.status, "same status");
    assert_eq!(a.header_names, b.header_names, "same headers");
    assert!(a.frames.is_empty() && a.first_event_after.is_none(), "{:?}", a.frames);
    assert_eq!(extract_structured(&a.response["result"]), json!({"warm": true}));
}

#[tokio::test]
async fn an_sse_call_on_mcp_delivers_its_first_event_before_the_stream_closes() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Elib AC4 sse").await;
    let admin = McpClient::with_bearer(&server.base_url, &key);
    admin
        .tools_call("host.tool_publish", json!({"name": "rotate", "kind": "python", "spec": {"source": TOOL}}))
        .await
        .expect("publish");
    let name = format!("{ns}.rotate");
    let client = RawClient::open(&server.base_url, &key, json!({"elicitation": {"form": {}}}), true).await;
    warm(&client, &name).await;

    // The client never answers, so the server holds the stream open for the
    // tool's own 3 s `timeout_s` after sending `elicitation/create`.
    let out = client.call(&name, json!({}), &silent).await;
    assert_eq!(out.content_type, "text/event-stream");
    let first = out.first_event_after.expect("an SSE reply has events");
    let gap = out.finished_after.saturating_sub(first);
    // A server-enforced wait (>= 2 s of the 3 s), not a performance budget:
    // buffered until close (R0 broken) the gap would be ~0.
    let held_open = gap >= Duration::from_secs(1);
    assert!(held_open, "the first event must arrive before the stream closes");
    // The uncaught `elicit_timeout` is the tool call's own error, in the same stream.
    assert!(out.response.to_string().contains("elicit_timeout"), "{}", out.response);
    assert_eq!(out.frames.len(), 1, "{:?}", out.frames);
    let _: &Value = &out.frames[0];
}
