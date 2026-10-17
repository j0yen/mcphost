//! PRD-mcphost-registry-entry-one-source AC1 (P0) -- Given a stub registry
//! answering the search with `dev.mcphost/mcphost` at 0.97.0 and a stub
//! endpoint whose `initialize` returns `serverInfo.version` 0.106.0, When
//! `registry-manifest --live` runs, Then it prints
//! `registry=0.97.0 live=0.106.0 remotes=<url> drift=yes` and exits 3; with
//! both at 0.106.0 it prints `drift=no` and exits 0.

use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

pub async fn stub_registry(version: &str, remote: &str) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0.1/servers/dev.mcphost%2Fmcphost/versions/latest"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "server": {"name": "dev.mcphost/mcphost", "version": version,
                       "remotes": [{"type": "streamable-http", "url": remote}]},
            "_meta": {}
        })))
        .mount(&server)
        .await;
    server
}

pub async fn stub_endpoint(version: &str) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0", "id": 1,
            "result": {"protocolVersion": "2025-06-18", "capabilities": {},
                       "serverInfo": {"name": "mcphost", "version": version}}
        })))
        .mount(&server)
        .await;
    server
}

pub async fn run_live(
    registry: &MockServer,
    endpoint: &MockServer,
    extra: &[&str],
) -> (i32, String) {
    let out = tokio::process::Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["registry-manifest", "--live", "--public-url", &endpoint.uri()])
        .args(extra)
        .env("MCPHOST_REGISTRY_SEARCH_URL", format!("{}/v0.1/servers/dev.mcphost%2Fmcphost/versions/latest", registry.uri()))
        .output()
        .await
        .expect("spawn mcphost registry-manifest --live");
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).to_string())
}

#[tokio::test]
async fn regone_ac01() {
    let endpoint = stub_endpoint("0.106.0").await;
    let remote = format!("{}/mcp", endpoint.uri());

    let registry = stub_registry("0.97.0", &remote).await;
    let (code, out) = run_live(&registry, &endpoint, &[]).await;
    assert_eq!(
        out.trim(),
        format!("registry=0.97.0 live=0.106.0 remotes={remote} drift=yes")
    );
    assert_eq!(code, 3);

    let registry = stub_registry("0.106.0", &remote).await;
    let (code, out) = run_live(&registry, &endpoint, &[]).await;
    assert_eq!(
        out.trim(),
        format!("registry=0.106.0 live=0.106.0 remotes={remote} drift=no")
    );
    assert_eq!(code, 0);
}
