//! PRD-mcphost-call-audit-export
//! AC3 (P0) — Given `host.calls.export` over 10 rows, When the CSV is
//! verified with the JWK named by `kid` from `/.well-known/jwks.json`, Then
//! the ES256 signature is valid and `sha256` matches the bytes; Given one
//! byte altered, Then verification fails.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use base64::Engine as _;
use jsonwebtoken::{Algorithm, DecodingKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// Verify `csv` against the bundle's signature with the JWK the published
/// JWKS names by `kid`, both the JWS-style `signature` and the DER
/// `signature_der` that `openssl dgst -verify` reads.
fn verifies(jwks: &Value, bundle: &Value, csv: &str) -> bool {
    let kid = bundle["kid"].as_str().expect("kid");
    let jwk = jwks["keys"]
        .as_array()
        .expect("keys")
        .iter()
        .find(|k| k["kid"] == json!(kid))
        .unwrap_or_else(|| panic!("kid {kid} is not in the JWKS: {jwks}"));
    let key = DecodingKey::from_ec_components(jwk["x"].as_str().expect("x"), jwk["y"].as_str().expect("y"))
        .expect("decoding key");
    let jws_ok = jsonwebtoken::crypto::verify(
        bundle["signature"].as_str().expect("signature"),
        csv.as_bytes(),
        &key,
        Algorithm::ES256,
    )
    .expect("verify call");

    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let mut point = vec![0x04];
    point.extend(b64.decode(jwk["x"].as_str().expect("x")).expect("x b64"));
    point.extend(b64.decode(jwk["y"].as_str().expect("y")).expect("y b64"));
    let der = base64::engine::general_purpose::STANDARD
        .decode(bundle["signature_der"].as_str().expect("signature_der"))
        .expect("der b64");
    let der_ok = ring::signature::UnparsedPublicKey::new(&ring::signature::ECDSA_P256_SHA256_ASN1, &point)
        .verify(csv.as_bytes(), &der)
        .is_ok();
    assert_eq!(jws_ok, der_ok, "the JWS and DER forms must agree");
    jws_ok
}

#[tokio::test]
async fn cae_ac03_export_signature_verifies_against_the_published_jwks_and_breaks_on_one_byte() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Export Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("find").expect("tenant");
    let now = mcphost::state::now_unix();
    for i in 0..10 {
        // A comma, a quote and a newline exercise the CSV quoting.
        let tool = if i == 0 { "odd,\"name\"\nx".to_string() } else { format!("tool_{i}") };
        server
            .state
            .db
            .insert_call_row_for_test_full(tenant.id, tool, None, Some("end user".to_string()), true, 5, now - 30)
            .await
            .expect("seed row");
    }

    let bundle = extract_structured(
        &client.tools_call("host.calls.export", json!({"since": "-1h"})).await.expect("export"),
    );
    assert_eq!(bundle["rows"], json!(10), "{bundle}");
    assert_eq!(bundle["alg"], json!("ES256"));
    let csv = bundle["csv"].as_str().expect("csv").to_string();

    // The header is the row struct's own field list, in order.
    let header = csv.lines().next().expect("header");
    let fields: Vec<&str> = header.split(',').collect();
    assert_eq!(fields, mcphost::calllog::CallRow::FIELDS);
    assert_eq!(
        fields,
        mcphost::paged::fields_for("host.calls.list").expect("projectable fields"),
        "CSV header, row struct and the projection list are one set"
    );
    assert!(csv.contains("\"odd,\"\"name\"\"\nx\""), "RFC 4180 quoting: {csv}");

    let sha: String = Sha256::digest(csv.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(bundle["sha256"], json!(sha), "sha256 matches the CSV bytes");

    let jwks: Value = reqwest::get(format!("{}/.well-known/jwks.json", server.base_url))
        .await
        .expect("GET jwks")
        .json()
        .await
        .expect("jwks json");
    assert!(verifies(&jwks, &bundle, &csv), "the signature must verify against the published JWK");

    // One byte altered: verification fails.
    let mut altered = csv.clone().into_bytes();
    let last = altered.len() - 2;
    altered[last] ^= 0x01;
    let altered = String::from_utf8(altered).expect("still utf-8");
    assert_ne!(altered, csv);
    assert!(!verifies(&jwks, &bundle, &altered), "an altered byte must break verification");
}
