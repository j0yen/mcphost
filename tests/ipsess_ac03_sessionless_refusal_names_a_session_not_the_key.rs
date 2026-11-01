//! PRD-mcphost-implicit-signup-per-session-not-per-peer
//! AC3 (P0) -- Given two requests from the same IP and User-Agent inside the
//! window, one with no `Mcp-Session-Id` and one with a forged id this process
//! never minted, When each calls `host.table.create` without `tenant_key`,
//! Then each error is `-32602` `tenant_key_missing`, `data.reason`
//! `peer_recent`, `data.session_presented` `false`, no `data.tenant`, and the
//! hint contains `initialize` and `/t/` but neither A's namespace nor
//! `pass the key`.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

const UA: &str = "ipsess-ac03/agent";

fn create_args() -> serde_json::Value {
    json!({"name": "runs", "columns": {"id": "integer"}})
}

#[tokio::test]
async fn ipsess_ac03() {
    let server = TestServer::start().await;

    let a = McpClient::new(&server.base_url).with_user_agent(UA).with_session_continuity();
    a.initialize().await;
    let signup = extract_structured(
        &a.tools_call("signup", json!({"name": "ipsess-a", "handoff": true})).await.expect("A signup"),
    );
    let a_namespace = signup["tenant"].as_str().expect("tenant").to_string();
    let token = signup["handoff_token"].as_str().expect("handoff_token").to_string();
    a.tools_call("host.redeem", json!({"handoff_token": token})).await.expect("A redeem");

    // No Mcp-Session-Id at all (a script that skipped `initialize`).
    let sessionless = McpClient::new(&server.base_url).with_user_agent(UA);
    let no_header = sessionless
        .tools_call("host.table.create", create_args())
        .await
        .expect_err("a sessionless request from the redeeming peer is refused");
    // An id this process never minted: the transport replaces it, never trusts it.
    let forged = McpClient::new(&server.base_url)
        .with_user_agent(UA)
        .tools_call_with_header("host.table.create", create_args(), ("Mcp-Session-Id", "forged-never-minted-0123456789abcdef"))
        .await
        .expect_err("a forged session id never turns the gate off");

    for (label, err) in [("no header", no_header), ("forged id", forged)] {
        assert_eq!(err.code, -32602, "{label}: {err:?}");
        assert_eq!(err.error_code.as_deref(), Some("tenant_key_missing"), "{label}: {err:?}");
        assert_eq!(err.data["reason"], json!("peer_recent"), "{label}: {err:?}");
        assert_eq!(err.data["session_presented"], json!(false), "{label}: {err:?}");
        assert!(err.data.get("tenant").is_none(), "{label}: {err:?}");
        let hint = err.data["hint"].as_str().expect("hint");
        assert!(hint.contains("initialize"), "{label}: {hint}");
        assert!(hint.contains("/t/"), "{label}: {hint}");
        assert!(!hint.contains(&a_namespace), "{label}: {hint}");
        assert!(!hint.contains("pass the key"), "{label}: {hint}");
        assert!(!format!("{err:?}").contains(&a_namespace), "{label}: no namespace anywhere: {err:?}");
    }
    assert_eq!(server.state.db.list_tenants().await.expect("tenants").len(), 1, "no tenant minted by a refusal");
}
