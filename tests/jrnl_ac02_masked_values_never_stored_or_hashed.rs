//! PRD-mcphost-session-journal-v2
//! AC2 (P0) — Given calls with `{"api_key":"k-1111","q":"x"}`,
//! `{"api_key":"k-2222","q":"x"}` and a string argument holding a tenant
//! secret's value of 8 characters, When listed, Then the first two share one
//! `args_sha256`, computed over `api_key` as `***` (the `Redactor` token),
//! the third hashes with the value as `{{ secret.NAME }}` (its token), and no
//! `calls` column holds `k-1111`, `k-2222` or the secret value.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use mcphost::idempotency::args_sha256;
use serde_json::json;

const SECRET_VALUE: &str = "s3cr3t-8";

#[tokio::test]
async fn jrnl_ac02_masked_values_never_stored_or_hashed() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Masked Fingerprint Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    assert_eq!(SECRET_VALUE.len(), 8);
    client
        .tools_call("host.secret_set", json!({"name": "VAULT", "value": SECRET_VALUE}))
        .await
        .expect("host.secret_set");
    client
        .tools_call(
            "host.tool.publish",
            json!({"name": "mask_echo", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    for args in [
        json!({"api_key": "k-1111", "q": "x"}),
        json!({"api_key": "k-2222", "q": "x"}),
        json!({"note": SECRET_VALUE}),
    ] {
        client.tools_call(&format!("{ns}.mask_echo"), args).await.expect("call");
    }

    let listing = extract_structured(
        &client
            .tools_call("host.calls.list", json!({"since": "-1h", "tool": "mask_echo"}))
            .await
            .expect("host.calls.list"),
    );
    let rows = listing["calls"].as_array().expect("calls");
    assert_eq!(rows.len(), 3, "{listing}");
    // Newest first: rows[2] is call 1.
    let sha = |i: usize| rows[2 - i]["args_sha256"].clone();
    assert_eq!(sha(0), sha(1), "two calls differing only in a masked value hash alike: {listing}");
    assert_eq!(sha(0), json!(args_sha256(&json!({"api_key": "***", "q": "x"}))), "{listing}");
    assert_eq!(sha(2), json!(args_sha256(&json!({"note": "{{ secret.VAULT }}"}))), "{listing}");

    let stored = server.state.db.calls_dump_for_test().await.expect("dump calls");
    assert!(!stored.is_empty(), "calls rows were written");
    for raw in ["k-1111", "k-2222", SECRET_VALUE] {
        assert!(!stored.contains(raw), "{raw} must not appear in any calls column: {stored}");
    }
}
