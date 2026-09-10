//! PRD-mcphost-provenance-audit
//! AC4 — Given healthz after migration, When fetched, Then tenant/signup/
//! call counts each appear as `{external, synthetic}` and no field
//! aggregates both under a "real" name.

mod common;
use common::ADMIN_KEY;
use serde_json::json;

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
async fn healthz_splits_tenants_signups_calls_and_drops_legacy_real_fields() {
    let server = common::TestServer::start().await;

    // One loopback (synthetic) signup via the real HTTP path, plus one
    // genuinely external signup via `control::signup` directly (a real
    // TCP connection to this test server is always loopback -- same
    // workaround `attrib_ac4`/`synthetic_ac07` use).
    common::signup(&server.base_url, "Loopback Persona").await;
    mcphost::control::signup(
        &server.state,
        &json!({"name": "Real External Co"}),
        "203.0.113.11",
        mcphost::control::SignupAttribution::default(),
    )
    .await
    .expect("external signup");

    let health = healthz(&server.base_url).await;

    for field in ["tenants", "signups", "calls"] {
        let obj = health.get(field).unwrap_or_else(|| panic!("missing '{field}' in {health:?}"));
        assert!(
            obj.get("external").and_then(serde_json::Value::as_i64).is_some(),
            "'{field}.external' missing or not an integer: {health:?}"
        );
        assert!(
            obj.get("synthetic").and_then(serde_json::Value::as_i64).is_some(),
            "'{field}.synthetic' missing or not an integer: {health:?}"
        );
    }
    assert_eq!(health["tenants"]["external"], json!(1), "{health:?}");
    assert_eq!(health["tenants"]["synthetic"], json!(1), "{health:?}");

    // Requirement 3: no field named "real" may aggregate both provenance
    // classes -- the old unqualified fields must be gone entirely, not
    // just renamed alongside the new ones.
    assert!(
        health.get("tenants_real").is_none(),
        "legacy 'tenants_real' must not be present: {health:?}"
    );
    assert!(
        health.get("tenants_synthetic").is_none(),
        "legacy top-level 'tenants_synthetic' must not be present: {health:?}"
    );
}
