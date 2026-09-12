//! AC4 — Given no `MCPHOST_STRIPE_SECRET_KEY`, When a tenant calls
//! `billing.checkout`, Then the error is `billing_unavailable` with
//! `billing_mode: off` and `/healthz` reports `billing_mode: "off"`.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn checkout_without_stripe_keys_is_billing_unavailable() {
    // `TestServer::start()` defaults to `BillingConfig::default()` -- no
    // keys configured, the documented "billing absent" state.
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "No Card").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call("billing.checkout", json!({}))
        .await
        .expect_err("checkout must fail with no Stripe keys configured");
    assert_eq!(err.error_code.as_deref(), Some("billing_unavailable"));
    assert_eq!(err.data["billing_mode"], json!("off"));

    // PRD-mcphost-healthz-minimal: `billing_mode` moved behind the admin
    // bearer -- the anonymous body is `{"ok": true/false}`.
    let health: serde_json::Value = reqwest::Client::new()
        .get(format!("{}/healthz", server.base_url))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .expect("GET /healthz")
        .json()
        .await
        .expect("parse /healthz");
    assert_eq!(health["billing_mode"], json!("off"));
}
