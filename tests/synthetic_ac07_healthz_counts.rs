//! PRD-mcphost-synthetic-flag
//! AC7 — Given 3 labeled and 2 unlabeled tenants, When `/healthz` is read,
//! Then `tenants_synthetic` is 3, `tenants_real` is 2, and `tenants_total`
//! is 5; and Given zero labeled tenants, Then both new keys are present
//! with `tenants_synthetic` 0.

mod common;
use common::{ADMIN_KEY, McpClient, TestServer, signup};
use serde_json::json;

// PRD-mcphost-healthz-minimal: the full diagnostics document (including
// `tenants_synthetic`/`tenants_real`/`tenants_total`) is gated behind the
// admin bearer -- an unauthenticated GET gets only `{"ok": true/false}`.
async fn healthz(base_url: &str) -> serde_json::Value {
    reqwest::Client::new()
        .get(format!("{base_url}/healthz"))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .expect("GET /healthz")
        .json()
        .await
        .expect("parse /healthz")
}

#[tokio::test]
async fn healthz_present_at_zero_and_splits_real_from_synthetic() {
    let server = TestServer::start().await;

    let health_empty = healthz(&server.base_url).await;
    assert_eq!(health_empty["tenants_synthetic"], json!(0), "{health_empty:?}");
    assert_eq!(health_empty["tenants_real"], json!(0), "{health_empty:?}");
    assert_eq!(health_empty["tenants_total"], json!(0), "{health_empty:?}");

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let mut labeled = Vec::new();
    for i in 0..3 {
        let (ns, _) = signup(&server.base_url, &format!("Synthetic {i}")).await;
        admin
            .tools_call(
                "admin.tenant_set_synthetic",
                json!({"tenant": ns, "label": "synthorg:ac7"}),
            )
            .await
            .expect("label tenant");
        labeled.push(ns);
    }
    for i in 0..2 {
        signup(&server.base_url, &format!("Real {i}")).await;
    }

    let health = healthz(&server.base_url).await;
    assert_eq!(health["tenants_total"], json!(5), "{health:?}");
    assert_eq!(health["tenants_synthetic"], json!(3), "{health:?}");
    assert_eq!(health["tenants_real"], json!(2), "{health:?}");
}
