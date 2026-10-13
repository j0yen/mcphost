//! Shared fixture for the `exprp_ac*` tests
//! (PRD-mcphost-example-from-published-tool): an http-kind server, one
//! tenant whose `probe` tool sends `Authorization: Bearer {{ secret.TOKEN }}`
//! to a mock upstream that only answers `{"probed": 1}` to the right token.

#![allow(dead_code)]

use crate::common::{McpClient, RpcError, TestServer, extract_structured, http_kind_registry, signup};
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

pub const TOKEN_VALUE: &str = "tok_live_exprp_canary_8841";
pub const PROOF_EXPECT: &str = "result.body.probed == 1";

pub struct Fixture {
    pub server: TestServer,
    pub upstream: MockServer,
    pub client: McpClient,
    pub ns: String,
}

pub async fn start() -> Fixture {
    start_with(http_kind_registry()).await
}

pub async fn start_with(kinds: mcphost::kinds::KindRegistry) -> Fixture {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/probe"))
        .and(header("Authorization", format!("Bearer {TOKEN_VALUE}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"probed": 1})))
        .mount(&upstream)
        .await;
    let server = TestServer::start_with_kinds(kinds).await;
    let (ns, key) = signup(&server.base_url, "exprp proposer").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client.tools_call("host.secret_set", json!({"name": "TOKEN", "value": TOKEN_VALUE})).await.expect("secret_set");
    Fixture { server, upstream, client, ns }
}

pub fn probe_spec(upstream: &MockServer) -> Value {
    json!({
        "method": "GET",
        "url": format!("{}/probe", upstream.uri()),
        "headers": {"Authorization": "Bearer {{ secret.TOKEN }}"},
        "args_schema": {"type": "object"},
        "description": "Probe the upstream with a stored token.",
    })
}

pub async fn publish_http(client: &McpClient, name: &str, spec: Value) {
    client
        .tools_call("host.tool_publish", json!({"name": name, "kind": "http", "spec": spec}))
        .await
        .unwrap_or_else(|e| panic!("publish {name}: {} {}", e.code, e.message));
}

pub fn propose_args(tool: &str) -> Value {
    json!({
        "tool": tool,
        "proof_call": {"args": {}, "expect": PROOF_EXPECT},
        "title": "Probe an upstream with a token",
        "use_case": ["uptime"],
    })
}

pub async fn propose(client: &McpClient, args: Value) -> Result<Value, RpcError> {
    client.tools_call("host.example.propose", args).await.map(|r| extract_structured(&r))
}
