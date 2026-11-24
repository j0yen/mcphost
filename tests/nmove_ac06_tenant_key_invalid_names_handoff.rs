//! PRD-mcphost-refusals-name-the-nearest-move
//! AC6 (P0) -- Given a `tenant_key` that matches no tenant, When the call
//! fails `tenant_key_invalid`, Then `next_call` is `{tool: "signup", args:
//! {handoff: true}}`, `hint` names `host.redeem` and `handoff_token`, no 8
//! bytes of the value appear in `message`, `data` or the journal, the
//! quickstart assertion is revised, and `reason` `handoff_token_as_key` is set
//! only if `crate::auth` prefixes handoff tokens.

use crate::common::{self, McpClient, RpcError, TestServer, extract_structured};
use serde_json::json;

/// True when any 8-byte window of `value` occurs in `haystack`.
fn leaks_eight_bytes(value: &str, haystack: &str) -> bool {
    value.as_bytes().windows(8).any(|w| std::str::from_utf8(w).is_ok_and(|w| haystack.contains(w)))
}

fn assert_handoff_path(err: &RpcError, value: &str, log: &str) {
    assert_eq!(err.error_code.as_deref(), Some("tenant_key_invalid"), "{err:?}");
    assert_eq!(err.data["next_call"], json!({"tool": "signup", "args": {"handoff": true}}), "{err:?}");
    let hint = err.data["hint"].as_str().expect("hint is a string");
    assert!(hint.contains("host.redeem") && hint.contains("handoff_token"), "{hint}");
    assert!(!leaks_eight_bytes(value, &err.message), "message: {}", err.message);
    assert!(!leaks_eight_bytes(value, &err.data.to_string()), "data: {}", err.data);
    assert!(!leaks_eight_bytes(value, log), "journal: {log}");
}

#[tokio::test]
async fn nmove_ac06() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);
    let journal = common::tracecap::capture_text();

    // A value that matches no tenant.
    let unknown = format!("t_{}", "9f3a1c7e5b2d4086".repeat(4));
    let err = client.tools_call("host.whoami", json!({"tenant_key": unknown})).await.expect_err("unknown key");
    assert_handoff_path(&err, &unknown, &journal.text());
    assert!(journal.text().contains("tenant_key_invalid"), "the capture sees the refusal's WARN line: {}", journal.text());
    assert_eq!(err.data["reason"], "unknown_key", "{err:?}");
    assert_ne!(err.data["next_call"]["tool"], "host.quickstart", "the generic quickstart row is gone");

    // A real handoff token presented as the key: the only case that sets `handoff_token_as_key`,
    // because `crate::auth` prefixes handoff tokens (`ho_`).
    let raw = client.tools_call("signup", json!({"name": "Nmove06", "handoff": true})).await.expect("signup handoff");
    let token = extract_structured(&raw)["handoff_token"].as_str().expect("handoff_token").to_string();
    assert!(token.starts_with(mcphost::auth::HANDOFF_TOKEN_PREFIX));
    let err = client.tools_call("host.whoami", json!({"tenant_key": token})).await.expect_err("token as key");
    assert_handoff_path(&err, &token, &journal.text());
    assert_eq!(err.data["reason"], "handoff_token_as_key", "{err:?}");
    // The token was not consumed by the refusal: it still redeems.
    client.tools_call("host.redeem", json!({"handoff_token": token})).await.expect("token still redeems");
}
