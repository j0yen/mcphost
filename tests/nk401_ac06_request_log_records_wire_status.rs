//! PRD-mcphost-named-key-refusal-not-http-401
//! AC6 -- the request log line for a ConnectionBound or PeerRecent refusal on
//! bare `/mcp` records status 200 (what the client received); a 401 on
//! `/login/mcp` records 401. `oauth_401_upgrade` runs inside the logger, so
//! the logged status is the post-decision one.

use crate::common;
use common::tracecap::Capture;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

fn last_status(buf: &Capture, path: &str) -> Option<u64> {
    buf.json_lines()
        .into_iter()
        .filter(|v| v.get("fields").and_then(|f| f.get("path")).and_then(|p| p.as_str()) == Some(path))
        .filter_map(|v| v["fields"]["status"].as_u64())
        .next_back()
}

fn whoami() -> serde_json::Value {
    json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
           "params": {"name": "host.whoami", "arguments": {}}})
}

#[tokio::test]
async fn nk401_ac06() {
    let server = TestServer::start().await;
    let buf = common::tracecap::capture_json();
    let base = &server.base_url;

    // ConnectionBound: a session that implicitly signed up, then a keyless call.
    let session = McpClient::new(base).with_session_continuity();
    session
        .tools_call(
            "host.tool_publish",
            json!({"name": "hello", "kind": "echo",
                   "spec": {"schema": {"type": "object", "properties": {}, "required": []}}}),
        )
        .await
        .expect("first call");
    let resp = session.post_with_mcp_name_override(whoami(), "host.whoami").await;
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body = resp.text().await.expect("body");
    assert!(body.contains("connection_bound"), "{body}");
    assert_eq!(last_status(&buf, "/mcp"), Some(200), "connection_bound logs 200");

    // PeerRecent: a redeem, then a keyless call with no session.
    let redeemer = McpClient::new(base);
    let signup = extract_structured(
        &redeemer.tools_call("signup", json!({"name": "R", "handoff": true})).await.expect("signup"),
    );
    let token = signup["handoff_token"].as_str().expect("token").to_string();
    redeemer.tools_call("host.redeem", json!({"handoff_token": token})).await.expect("redeem");
    let resp = McpClient::new(base).post_with_mcp_name_override(whoami(), "host.whoami").await;
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body = resp.text().await.expect("body");
    assert!(body.contains("peer_recent"), "{body}");
    assert_eq!(last_status(&buf, "/mcp"), Some(200), "peer_recent logs 200");

    // /login/mcp stays 401, and the log says so.
    let resp = McpClient::new(base)
        .with_path("/login/mcp")
        .post_with_mcp_name_override(whoami(), "host.whoami")
        .await;
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
    assert_eq!(last_status(&buf, "/login/mcp"), Some(401), "/login/mcp logs 401");
}
