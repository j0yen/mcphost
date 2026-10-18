//! PRD-mcphost-elicitation-in-band-on-stateless-transport
//! AC6 (P0) — Given an in-band elicitation in flight, When the client never
//! answers, Then `elicit_timeout` is returned at `timeout_s` inside the tool's
//! budget and the stream closes with the tool's result; Given a POSTed
//! response whose request id is unknown or already completed, or whose
//! `Mcp-Session-Id` belongs to another session, Then the server answers `202`,
//! delivers nothing, and the in-flight call is unaffected.

use crate::inband_client;

use crate::common;
use common::{McpClient, TestServer, extract_structured, python_kind_registry, signup};
use inband_client::{RawClient, accept, silent};
use mcphost::sandbox;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const TOOL: &str = "import mcphost\ndef main(args):\n    if args.get(\"warm\"):\n        return {\"warm\": True}\n    return mcphost.elicit(\"Rotate?\", {\"confirm\": {\"type\": \"boolean\"}}, timeout_s=args.get(\"timeout_s\"))\n";

async fn fixture(label: &str) -> (TestServer, common::TempDataDir, String, String) {
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, label).await;
    let admin = McpClient::with_bearer(&server.base_url, &key);
    admin
        .tools_call("host.tool_publish", json!({"name": "rotate", "kind": "python", "spec": {"source": TOOL}}))
        .await
        .expect("publish");
    (server, envs_dir, format!("{ns}.rotate"), key)
}

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
async fn a_silent_client_gets_elicit_timeout_at_timeout_s_and_the_stream_closes_with_the_result() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let (server, _envs, name, key) = fixture("Elib AC6 silent").await;
    let client = RawClient::open(&server.base_url, &key, json!({"elicitation": {"form": {}}}), true).await;
    warm(&client, &name).await;

    let out = crate::deadline_budget!(15_000, at_least = 900, { client.call(&name, json!({"timeout_s": 1}), &silent).await });
    assert_eq!(out.content_type, "text/event-stream");
    assert_eq!(out.frames.len(), 1, "the client was asked once: {:?}", out.frames);
    let err = out.response.to_string();
    assert!(err.contains("elicit_timeout"), "{err}");
    assert!(err.contains("did not answer within 1s"), "{err}");

    // The slot is free again: the next elicit on the same session is in-band.
    let again = client.call(&name, json!({"timeout_s": 5}), &|_: &Value| accept(json!({"confirm": true}))).await;
    assert_eq!(again.frames.len(), 1);
    assert_eq!(extract_structured(&again.response["result"])["action"], json!("accept"), "{}", again.response);
}

#[tokio::test]
async fn unknown_completed_and_foreign_session_answers_are_202_and_deliver_nothing() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let (server, _envs, name, key) = fixture("Elib AC6 stray").await;
    let caller = Arc::new(RawClient::open(&server.base_url, &key, json!({"elicitation": {"form": {}}}), true).await);
    let other = RawClient::open(&server.base_url, &key, json!({"elicitation": {"form": {}}}), true).await;
    warm(&caller, &name).await;

    // The real answer arrives 2 s after the frame; the stray ones go in first.
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
    // (a) the in-flight session, an id nobody asked
    let unknown = caller.post(&json!({"jsonrpc": "2.0", "id": 424242, "result": wrong}), None, None).await;
    assert_eq!(unknown.status(), 202);
    // (b) the real id, but under another session's `Mcp-Session-Id`
    let foreign = other.post(&json!({"jsonrpc": "2.0", "id": id, "result": wrong}), None, None).await;
    assert_eq!(foreign.status(), 202);
    // (c) the real id under a session id the host never minted
    let unminted = caller.post(&json!({"jsonrpc": "2.0", "id": id, "result": wrong}), None, Some("not-a-minted-id")).await;
    assert_eq!(unminted.status(), 202);

    let out = call.await.expect("join");
    assert_eq!(
        extract_structured(&out.response["result"]),
        json!({"action": "accept", "content": {"confirm": true}}),
        "stray answers must not reach the call: {}",
        out.response
    );
    assert_eq!(out.answer_statuses, vec![202]);

    // (d) the same id again, now that the request has completed
    let done = caller.post(&json!({"jsonrpc": "2.0", "id": id, "result": wrong}), None, None).await;
    assert_eq!(done.status(), 202);
}
