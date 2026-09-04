//! AC8 — Given the running server, When any client completes `initialize`,
//! Then the response carries `MCP-Protocol-Version: 2025-11-25`, matching
//! the `protocolVersion` in the `initialize` result body.
//! AC9 — Given the source tree, When a test compares the advertised header
//! value against `rmcp::model::ProtocolVersion::LATEST.as_str()`, Then
//! they are equal, and no string literal `"2026-07-28"` remains as the
//! advertised version in `src/http.rs`.
//! AC10 — Given the advertised version, When a test compares it against
//! `rmcp::model::ProtocolVersion::STANDARD_HEADERS.as_str()`, Then the
//! advertised version is strictly lower, so SEP-2243 header validation is
//! never triggered by mcphost's own advertisement.

mod common;
use common::TestServer;
use rmcp::model::ProtocolVersion;

#[tokio::test]
async fn ac8_initialize_header_matches_negotiated_protocol_version() {
    let server = TestServer::start().await;
    let http = reqwest::Client::new();

    let resp = http
        .post(format!("{}/mcp", server.base_url))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "ac8-test", "version": "0.1"}
            }
        }))
        .send()
        .await
        .expect("send initialize");

    let header_value = resp
        .headers()
        .get("MCP-Protocol-Version")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    assert_eq!(
        header_value.as_deref(),
        Some("2025-11-25"),
        "the initialize response must advertise 2025-11-25"
    );

    let body: serde_json::Value = resp.json().await.expect("parse initialize response");
    let negotiated = body["result"]["protocolVersion"]
        .as_str()
        .expect("initialize result carries protocolVersion");
    assert_eq!(
        header_value.as_deref(),
        Some(negotiated),
        "the header must match the protocolVersion actually negotiated in the body"
    );
}

#[tokio::test]
async fn ac9_advertised_version_equals_rmcp_latest_and_no_literal_remains() {
    let server = TestServer::start().await;
    let http = reqwest::Client::new();

    let resp = http
        .get(format!("{}/healthz", server.base_url))
        .send()
        .await
        .expect("send healthz");
    let advertised = resp
        .headers()
        .get("MCP-Protocol-Version")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .expect("healthz response carries MCP-Protocol-Version (the layer is router-wide)");

    assert_eq!(
        advertised,
        ProtocolVersion::LATEST.as_str(),
        "the advertised version must be derived from rmcp::model::ProtocolVersion::LATEST"
    );

    // The other half of AC9: no `"2026-07-28"` string literal remains as
    // the advertised version in the source. This greps the actual source
    // file rather than trusting the runtime check above, because a
    // literal could exist unused (e.g. a still-present but unreferenced
    // `const`) and pass the behavioural assertion above while still
    // reintroducing the defect the moment someone wires it back in.
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/http.rs"))
        .expect("read src/http.rs");
    assert!(
        !src.contains("\"2026-07-28\""),
        "src/http.rs must not contain the string literal \"2026-07-28\" as an advertised version"
    );
}

#[tokio::test]
async fn ac10_advertised_version_is_strictly_below_standard_headers() {
    let server = TestServer::start().await;
    let http = reqwest::Client::new();

    let resp = http
        .get(format!("{}/healthz", server.base_url))
        .send()
        .await
        .expect("send healthz");
    let advertised = resp
        .headers()
        .get("MCP-Protocol-Version")
        .and_then(|v| v.to_str().ok())
        .expect("healthz response carries MCP-Protocol-Version")
        .to_string();

    assert!(
        advertised.as_str() < ProtocolVersion::STANDARD_HEADERS.as_str(),
        "advertised version {advertised} must be strictly lower than STANDARD_HEADERS ({}) \
         so mcphost's own advertisement never triggers SEP-2243 header validation",
        ProtocolVersion::STANDARD_HEADERS.as_str()
    );
}
