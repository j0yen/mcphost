//! PRD-mcphost-chain-friction-fixes-20261016 AC4 (P0) -- `host.trigger.test`
//! on a webhook trigger self-signs every scheme `verify_delivery` accepts,
//! because both read the one `SCHEMES` table.

use crate::common;
use axum::http::{HeaderMap, HeaderName, HeaderValue};
use common::{extract_structured, signup};
use mcphost::webhooks::{SCHEMES, verify_delivery};
use serde_json::{Value, json};

fn headers_with(header: &str, value: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(HeaderName::from_bytes(header.as_bytes()).unwrap(), HeaderValue::from_str(value).unwrap());
    h
}

/// A scheme accepted by the verifier but not signed by the test endpoint's
/// signer is a failing test: every scheme's own signer output verifies, and
/// (for a signing scheme) fails under a different secret.
#[test]
fn every_scheme_signer_output_verifies() {
    assert!(SCHEMES.iter().any(|s| s.name == "github"));
    let body = br#"{"ref":"refs/heads/main"}"#;
    for scheme in SCHEMES {
        let headers = match scheme.header {
            Some(h) => headers_with(h, &(scheme.sign)("s3cr3t", body, mcphost::state::now_unix())),
            None => HeaderMap::new(),
        };
        verify_delivery(scheme.name, "s3cr3t", body, &headers)
            .unwrap_or_else(|e| panic!("{}: signer output rejected: {e:?}", scheme.name));
        if scheme.header.is_some() {
            assert!(
                verify_delivery(scheme.name, "other-secret", body, &headers).is_err(),
                "{}: a different secret must not verify",
                scheme.name
            );
            assert!(
                verify_delivery(scheme.name, "s3cr3t", body, &HeaderMap::new()).is_err(),
                "{}: a missing header must not verify",
                scheme.name
            );
        }
    }
    assert!(verify_delivery("not-a-scheme", "s", body, &HeaderMap::new()).is_err());
}

#[tokio::test]
async fn trigger_test_with_no_headers_runs_for_every_scheme_and_stores_the_signature_header() {
    let server = common::TestServer::start().await;
    for scheme in SCHEMES {
        // A fresh tenant per scheme: the plan caps a tenant at 3 triggers.
        let (_ns, key) = signup(&server.base_url, &format!("Cdft AC4 {}", scheme.name)).await;
        let client = common::McpClient::with_bearer(&server.base_url, &key);
        client
            .tools_call(
                "host.tool_publish",
                json!({"name": "on_hook", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
            )
            .await
            .expect("publish");
        let name = format!("hook_{}", scheme.name);
        let set = extract_structured(
            &client
                .tools_call(
                    "host.trigger.set",
                    json!({"tool": "on_hook", "kind": "webhook", "name": name, "verify": scheme.name}),
                )
                .await
                .unwrap_or_else(|e| panic!("{}: trigger.set: {e:?}", scheme.name)),
        );
        let id = set["id"].as_str().expect("id").to_string();
        let tested = extract_structured(
            &client
                .tools_call("host.trigger.test", json!({"id": id, "body": {"ref": "refs/heads/main"}}))
                .await
                .unwrap_or_else(|e| panic!("{}: trigger.test refused: {e:?}", scheme.name)),
        );
        assert!(tested["row_id"].is_number(), "{}: {tested}", scheme.name);
        assert!(tested["run_id"].is_string(), "{}: {tested}", scheme.name);

        let rows = extract_structured(
            &client
                .tools_call("host.state.query", json!({"table": format!("inbox_{name}")}))
                .await
                .expect("state.query"),
        );
        let stored: &Value = &rows["rows"][0]["headers"];
        if let Some(header) = scheme.header {
            assert!(
                stored.get(header.to_ascii_lowercase()).is_some(),
                "{}: stored delivery headers lack {header}: {stored}",
                scheme.name
            );
        }
    }
}
