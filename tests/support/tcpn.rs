//! PRD-mcphost-tools-call-progress-notifications: shared fixture for the tcpn
//! tests (token-bearing `tools/call`, origin-split signup, admin healthz).
//!
//! Not a crate module: pulled in per-file with `#[path = "support/tcpn.rs"]
//! mod tcpn;`, same convention as `tests/support/host.rs`.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer};
use serde_json::{Value, json};

/// A `tools/call` of `name` with an optional `_meta.progressToken`.
pub async fn call_with_token(client: &McpClient, name: &str, args: Value, token: Option<Value>) -> reqwest::Response {
    let mut meta = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": {"name": "tcpn", "version": "1"},
    });
    if let Some(token) = token {
        meta["progressToken"] = token;
    }
    client
        .post_raw(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"name": name, "arguments": args, "_meta": meta},
        }))
        .await
}

/// A signed-up tenant from `ip` (loopback is `synthetic`, a public address
/// `external`) with a published `echo` tool, which never reports progress.
pub async fn signup(server: &TestServer, name: &str, ip: &str) -> (McpClient, String) {
    let result = mcphost::control::signup(&server.state, &json!({"name": name}), ip, mcphost::control::SignupAttribution::default())
        .await
        .unwrap_or_else(|e| panic!("signup '{name}': {e:?}"));
    let client = McpClient::with_bearer(&server.base_url, result["key"].as_str().expect("key"));
    let published = common::publish(&client, "hello", "echo", json!({"schema": {"type": "object"}})).await;
    (client, published)
}

#[allow(dead_code)]
pub async fn admin_healthz(base_url: &str) -> Value {
    reqwest::Client::new().get(format!("{base_url}/healthz")).bearer_auth(ADMIN_KEY).send().await.expect("GET /healthz").json().await.expect("parse /healthz")
}

/// Seeds the AC1 fixture and returns the server.
pub async fn ac1_fixture() -> TestServer {
    let server = TestServer::start_with_signup_rate_limit(20).await;
    let (synthetic, synthetic_tool) = signup(&server, "tcpn synthetic", "127.0.0.1").await;
    let (external, external_tool) = signup(&server, "tcpn external", "203.0.113.11").await;
    for (client, tool, token) in [
        (&synthetic, &synthetic_tool, Some(json!("a"))),
        (&synthetic, &synthetic_tool, None),
        (&external, &external_tool, Some(json!("b"))),
    ] {
        let resp = call_with_token(client, tool, json!({"msg": "hi"}), token).await;
        assert!(resp.status().is_success(), "{tool}: {}", resp.status());
        let body = common::parse_response_body(resp).await;
        assert!(body.get("error").is_none(), "{body}");
    }
    server
}
