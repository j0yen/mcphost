//! PRD-mcphost-call-audit-export
//! AC7 (P0) — Given an empty window, When `host.calls.export` runs, Then
//! `rows == 0`, `csv` is the header line only, and the signature still
//! verifies.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use jsonwebtoken::{Algorithm, DecodingKey};
use serde_json::{Value, json};

#[tokio::test]
async fn cae_ac07_empty_window_is_the_header_line_and_still_signed() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Empty Window Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    // A window that ended before this tenant existed.
    let bundle = extract_structured(
        &client
            .tools_call("host.calls.export", json!({"since": "2020-01-01", "until": "2020-01-02"}))
            .await
            .expect("export an empty window"),
    );
    assert_eq!(bundle["rows"], json!(0), "{bundle}");
    let csv = bundle["csv"].as_str().expect("csv");
    let header = mcphost::calllog::CallRow::FIELDS.join(",") + "\n";
    assert_eq!(csv, header, "the CSV is the header line and nothing else");

    let sha: String = {
        use sha2::{Digest, Sha256};
        Sha256::digest(csv.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
    };
    assert_eq!(bundle["sha256"], json!(sha));

    let jwks: Value = reqwest::get(format!("{}/.well-known/jwks.json", server.base_url))
        .await
        .expect("GET jwks")
        .json()
        .await
        .expect("jwks json");
    let jwk = jwks["keys"]
        .as_array()
        .expect("keys")
        .iter()
        .find(|k| k["kid"] == bundle["kid"])
        .unwrap_or_else(|| panic!("kid not in JWKS: {jwks}"));
    let decoding = DecodingKey::from_ec_components(jwk["x"].as_str().expect("x"), jwk["y"].as_str().expect("y"))
        .expect("decoding key");
    let verify = |bytes: &str| {
        jsonwebtoken::crypto::verify(bundle["signature"].as_str().expect("signature"), bytes.as_bytes(), &decoding, Algorithm::ES256)
            .expect("verify call")
    };
    assert!(verify(csv), "the header-only CSV must verify");
    assert!(!verify("ts,credential\n"), "a different byte string must not verify");
}
