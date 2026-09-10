//! PRD-mcphost-synthetic-flag
//! AC7 — Given 3 labeled and 2 unlabeled tenants, When `/healthz` is read,
//! Then `tenants_synthetic` and `tenants_real` are both present (0 when
//! none) and `tenants_total` is exact.
//!
//! PRD-mcphost-tenant-attribution requirement 3 corrects what "real" means
//! here: every signup this suite's test server makes is loopback-sourced,
//! so an *unlabeled* loopback signup is still synthetic
//! (`harness:unstamped`), not real -- `tenants_real` only ever counts
//! `source_class = external`. This file keeps AC7's original shape
//! (present-at-zero, then a labeled/unlabeled mix) and adds one genuinely
//! `external` signup (via `control::signup` directly, the same way
//! `attrib_ac4` does, since a real TCP connection to this test server is
//! always loopback) so the split is demonstrated correctly rather than
//! vacuously.

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
    assert_eq!(health_empty["tenants"]["synthetic"], json!(0), "{health_empty:?}");
    assert_eq!(health_empty["tenants"]["external"], json!(0), "{health_empty:?}");
    assert_eq!(health_empty["tenants_total"], json!(0), "{health_empty:?}");

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    for i in 0..3 {
        let (ns, _) = signup(&server.base_url, &format!("Synthetic {i}")).await;
        admin
            .tools_call(
                "admin.tenant_set_synthetic",
                json!({"tenant": ns, "label": "synthorg:ac7"}),
            )
            .await
            .expect("label tenant");
    }
    // Two more loopback signups with no explicit label -- still synthetic
    // (`harness:unstamped`) under the corrected definition, not real.
    for i in 0..2 {
        signup(&server.base_url, &format!("Unlabeled Loopback {i}")).await;
    }
    // One genuinely external signup: bypasses the HTTP layer (a real
    // connection to this test server is always 127.0.0.1) the same way
    // `attrib_ac4_external_signup_counts_real.rs` does.
    mcphost::control::signup(
        &server.state,
        &json!({"name": "Joe's Real Company"}),
        "8.8.8.8",
        mcphost::control::SignupAttribution::default(),
    )
    .await
    .expect("external signup");

    let health = healthz(&server.base_url).await;
    assert_eq!(health["tenants_total"], json!(6), "{health:?}");
    assert_eq!(health["tenants"]["synthetic"], json!(5), "{health:?}");
    assert_eq!(health["tenants"]["external"], json!(1), "{health:?}");
}
