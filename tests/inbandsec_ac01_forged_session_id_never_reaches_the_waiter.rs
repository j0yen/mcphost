//! In-band security: a client JSON-RPC response carrying a forged
//! `Mcp-Session-Id` -- the real session's nonce with a flipped tag digit, the
//! sharpest forgery -- while a waiter is in flight under the real session
//! must not resolve the waiter, and the host replaces the forged id (the
//! reply's `Mcp-Session-Id` differs from the forged one). The real answer
//! then still reaches the call. Unit-level pins: `inbandsec_*` in
//! `src/inband.rs` and `src/session_bind.rs`.

use crate::inband_client;

use crate::common;
use common::{McpClient, TestServer, extract_structured, python_kind_registry, signup};
use inband_client::RawClient;
use mcphost::sandbox;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const TOOL: &str = "import mcphost\ndef main(args):\n    if args.get(\"warm\"):\n        return {\"warm\": True}\n    return mcphost.elicit(\"Rotate?\", {\"confirm\": {\"type\": \"boolean\"}}, timeout_s=args.get(\"timeout_s\"))\n";

#[tokio::test]
async fn inbandsec_forged_session_id_never_reaches_the_waiter_and_is_replaced() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Inbandsec forged").await;
    McpClient::with_bearer(&server.base_url, &key)
        .tools_call("host.tool_publish", json!({"name": "rotate", "kind": "python", "spec": {"source": TOOL}}))
        .await
        .expect("publish");
    let name = format!("{ns}.rotate");
    let caller = Arc::new(RawClient::open(&server.base_url, &key, json!({"elicitation": {"form": {}}}), true).await);
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

    // The real session's nonce, its tag's last digit flipped.
    let real = caller.session.clone();
    let flipped = if real.ends_with('0') { '1' } else { '0' };
    let forged = format!("{}{flipped}", &real[..real.len() - 1]);
    assert_ne!(forged, real);
    let wrong = json!({"action": "accept", "content": {"confirm": false}});
    let resp = caller.post(&json!({"jsonrpc": "2.0", "id": id, "result": wrong}), None, Some(&forged)).await;
    assert_eq!(resp.status(), 202, "a forged-session answer is dropped like any stray");
    if let Some(echoed) = resp.headers().get("mcp-session-id") {
        let echoed = echoed.to_str().expect("header is text");
        assert_ne!(echoed, forged, "the forged id is replaced, never echoed");
        assert_ne!(echoed, real, "and not swapped for the real session either");
    }

    let out = call.await.expect("join");
    assert_eq!(
        extract_structured(&out.response["result"]),
        json!({"action": "accept", "content": {"confirm": true}}),
        "the forged answer must not resolve the waiter: {}",
        out.response
    );
}
