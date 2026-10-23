//! PRD-mcphost-named-key-refusal-not-http-401
//! AC2 -- a keyless host.whoami with no `Mcp-Session-Id`, same peer as a
//! redeem, within 600 s on bare /mcp: HTTP 200, no WWW-Authenticate, -32602
//! `tenant_key_missing` with `data.reason` peer_recent and NO `data.tenant`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, parse_response_body};
use serde_json::json;

#[tokio::test]
async fn nk401_ac02() {
    let server = TestServer::start().await;
    let redeemer = McpClient::new(&server.base_url);
    let signup = extract_structured(
        &redeemer.tools_call("signup", json!({"name": "Redeemer", "handoff": true})).await.expect("signup"),
    );
    let namespace = signup["tenant"].as_str().expect("tenant").to_string();
    let token = signup["handoff_token"].as_str().expect("handoff_token").to_string();
    redeemer.tools_call("host.redeem", json!({"handoff_token": token})).await.expect("redeem");

    // A fresh stateless client: same IP and User-Agent, no session id.
    let resp = McpClient::new(&server.base_url)
        .post_with_mcp_name_override(
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                   "params": {"name": "host.whoami", "arguments": {}}}),
            "host.whoami",
        )
        .await;
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    assert!(resp.headers().get(reqwest::header::WWW_AUTHENTICATE).is_none());
    let body = parse_response_body(resp).await;
    assert_eq!(body["error"]["code"], json!(-32602), "{body}");
    let data = &body["error"]["data"];
    assert_eq!(data["error_code"], json!("tenant_key_missing"), "{body}");
    assert_eq!(data["reason"], json!("peer_recent"), "{body}");
    assert!(data.get("tenant").is_none(), "no tenant key on the wire: {body}");
    assert!(!body.to_string().contains(&namespace), "namespace must not leak: {body}");
    assert_eq!(data["hint"], json!(mcphost::errors::PEER_RECENT_HINT), "{body}");
}
