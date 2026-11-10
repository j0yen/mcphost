//! rmcp 3.6+ answers a POSTed JSON-RPC response on its stateless transport
//! `400` (rust-sdk #1326, not RUSTSEC-2026-0461/0462); mcphost keeps the `202`
//! its in-band contract states (elib AC6). The shim in
//! `http::refuse_unimplemented_methods` matches one body shape only, runs
//! after the body limit and the route's own gates, and delivers nothing:
//! delivery stays in `deliver_client_response`, keyed by a session this
//! process minted that has a request in flight.

use crate::inband_client;

use crate::common;
use common::{McpClient, TestServer, extract_structured, python_kind_registry, signup};
use inband_client::RawClient;
use mcphost::sandbox;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const TOOL: &str = "import mcphost\ndef main(args):\n    if args.get(\"warm\"):\n        return {\"warm\": True}\n    return mcphost.elicit(\"Rotate?\", {\"confirm\": {\"type\": \"boolean\"}}, timeout_s=args.get(\"timeout_s\"))\n";

async fn raw_post(base: &str, path: &str, bearer: Option<&str>, session: Option<&str>, body: Vec<u8>) -> reqwest::Response {
    let mut req = reqwest::Client::new()
        .post(format!("{base}{path}"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(body);
    if let Some(key) = bearer {
        req = req.header("Authorization", format!("Bearer {key}"));
    }
    if let Some(session) = session {
        req = req.header("Mcp-Session-Id", session);
    }
    req.send().await.expect("post")
}

fn response_body(extra: Value) -> Vec<u8> {
    let mut doc = json!({"jsonrpc": "2.0", "id": 424242, "result": {"action": "accept", "content": {"confirm": false}}});
    for (k, v) in extra.as_object().expect("object") {
        doc[k] = v.clone();
    }
    serde_json::to_vec(&doc).expect("json")
}

#[tokio::test]
async fn a_lone_response_object_is_202_and_every_other_shape_is_left_to_the_transport() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Inbandsec shim shapes").await;
    let base = &server.base_url;

    let lone = raw_post(base, "/mcp", Some(&key), None, response_body(json!({}))).await;
    assert_eq!(lone.status(), 202);
    assert!(lone.bytes().await.expect("body").is_empty(), "202 carries no body");
    let error = json!({"jsonrpc": "2.0", "id": "abc", "error": {"code": -32000, "message": "no"}});
    let lone_error = raw_post(base, "/mcp", Some(&key), None, serde_json::to_vec(&error).unwrap()).await;
    assert_eq!(lone_error.status(), 202);

    // Both result and error: not a response object; the transport rejects it.
    let both = raw_post(base, "/mcp", Some(&key), None, response_body(json!({"error": {"code": -1, "message": "x"}}))).await;
    assert_ne!(both.status(), 202, "a result-and-error body must not be accepted by the shim");
    assert!(both.status().is_client_error(), "rmcp's own refusal stands: {}", both.status());
    // Neither result nor error, a method, a batch: not shimmed.
    let neither = raw_post(base, "/mcp", Some(&key), None, br#"{"jsonrpc":"2.0","id":1}"#.to_vec()).await;
    assert_ne!(neither.status(), 202);
    let batch = raw_post(base, "/mcp", Some(&key), None, serde_json::to_vec(&json!([{"jsonrpc": "2.0", "id": 1, "result": {}}])).unwrap()).await;
    assert_ne!(batch.status(), 202, "a batch passes to the transport");

    // A request with a method reaches the handler unchanged.
    let list = McpClient::with_bearer(base, &key).tools_list().await;
    assert!(list.is_ok(), "tools/list still answered by the handler: {list:?}");
    let with_method = raw_post(
        base,
        "/mcp",
        Some(&key),
        None,
        serde_json::to_vec(&json!({"jsonrpc": "2.0", "id": 7, "method": "ping", "result": {}})).unwrap(),
    )
    .await;
    assert_ne!(with_method.status(), 202, "a body with a method is never taken for a response");
}

#[tokio::test]
async fn an_oversized_response_body_is_refused_by_the_existing_body_limit() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Inbandsec shim oversize").await;
    let pad = "x".repeat(mcphost::state::MAX_REQUEST_BODY_BYTES + 1024);
    let resp = raw_post(&server.base_url, "/mcp", Some(&key), None, response_body(json!({"pad": pad}))).await;
    assert_eq!(resp.status(), 413, "the 1 MiB limit applies before the shim");
}

#[tokio::test]
async fn an_unauthenticated_response_is_stopped_by_the_route_gate_and_delivers_nothing() {
    let server = TestServer::start().await;
    let base = &server.base_url;
    // The entry route's bearer gate sits outside the shim: 401, as before.
    let gated = raw_post(base, mcphost::oauth::OAUTH_ENTRY_PATH, None, None, response_body(json!({}))).await;
    assert_eq!(gated.status(), 401, "require_entry_bearer still answers first");
    // `/mcp` has no bearer gate ahead of the transport for a response (it
    // never reached the handler's auth under rmcp <= 3.5 either): 202, and
    // with no session of its own in flight nothing is delivered.
    let open = raw_post(base, "/mcp", None, None, response_body(json!({}))).await;
    assert_eq!(open.status(), 202);
}

#[tokio::test]
async fn forged_and_foreign_tenant_sessions_never_deliver_and_the_real_answer_still_does() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Inbandshim owner").await;
    let (_other_ns, other_key) = signup(&server.base_url, "Inbandshim foreign").await;
    McpClient::with_bearer(&server.base_url, &key)
        .tools_call("host.tool_publish", json!({"name": "rotate", "kind": "python", "spec": {"source": TOOL}}))
        .await
        .expect("publish");
    let name = format!("{ns}.rotate");
    let caller = Arc::new(RawClient::open(&server.base_url, &key, json!({"elicitation": {"form": {}}}), true).await);
    let foreign = RawClient::open(&server.base_url, &other_key, json!({"elicitation": {"form": {}}}), true).await;
    for _ in 0..200 {
        let out = caller.call(&name, json!({"warm": true}), &inband_client::silent).await;
        if out.response["result"]["structuredContent"]["warm"] == json!(true) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let seen: Arc<Mutex<Option<Value>>> = Arc::default();
    let seen_in = seen.clone();
    let call = {
        let (caller, name) = (caller.clone(), name.clone());
        tokio::spawn(async move {
            caller
                .call(&name, json!({"timeout_s": 20}), &move |frame: &Value| {
                    *seen_in.lock().unwrap() = Some(frame["id"].clone());
                    Some((Duration::from_secs(2), json!({"action": "accept", "content": {"confirm": true}})))
                })
                .await
        })
    };
    let id = loop {
        if let Some(id) = seen.lock().unwrap().clone() {
            break id;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let wrong = json!({"action": "accept", "content": {"confirm": false}});
    let body = json!({"jsonrpc": "2.0", "id": id, "result": wrong});
    // The foreign tenant, under its own session, with the live request id.
    assert_eq!(foreign.post(&body, None, None).await.status(), 202);
    // The foreign tenant's bearer carrying the owner's real session id.
    let real = caller.session.clone();
    let crossed = raw_post(&server.base_url, "/mcp", Some(&other_key), Some(&real), serde_json::to_vec(&body).unwrap()).await;
    assert_eq!(crossed.status(), 202);
    // A session id the host never minted.
    assert_eq!(caller.post(&body, None, Some("forged-session-id")).await.status(), 202);

    let out = call.await.expect("join");
    assert_eq!(
        extract_structured(&out.response["result"]),
        json!({"action": "accept", "content": {"confirm": true}}),
        "no stray answer may resolve the waiter: {}",
        out.response
    );
}
