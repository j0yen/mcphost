//! PRD-mcphost-activation-funnel
//! AC4 (P0) — Given `/healthz`, When read, Then `funnel_7d` lists the
//! external stage counts and no synthetic tenant is counted.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer};
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
async fn funnel_7d_counts_external_tenants_only() {
    let server = TestServer::start().await;

    // One external tenant that publishes (a real authenticated call, so
    // both `first_call` and `first_publish` should show up).
    let signed_up = mcphost::control::signup(
        &server.state,
        &json!({"name": "AC4 External"}),
        "8.8.8.8",
        mcphost::control::SignupAttribution::default(),
    )
    .await
    .expect("external signup");
    let key = signed_up["key"].as_str().expect("key").to_string();
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "funnel_ac04_tool",
                "kind": "echo",
                "spec": {"schema": {"type": "object"}},
            }),
        )
        .await
        .expect("external tenant publishes");

    // Several synthetic (loopback) signups -- must never be counted.
    for i in 0..5 {
        common::signup(&server.base_url, &format!("AC4 Synthetic {i}")).await;
    }

    let health = healthz(&server.base_url).await;
    let funnel_7d = health.get("funnel_7d").unwrap_or_else(|| panic!("missing funnel_7d in {health}"));
    assert_eq!(
        funnel_7d["signups"],
        json!(1),
        "only the one external signup must be counted, not the 5 synthetic ones: {funnel_7d}"
    );
    assert_eq!(funnel_7d["first_call"], json!(1), "{funnel_7d}");
    assert_eq!(funnel_7d["first_publish"], json!(1), "{funnel_7d}");
    // Requirement 4: counts only -- no medians at this altitude.
    assert!(
        funnel_7d.get("median_minutes").is_none() && funnel_7d.get("stages").is_none(),
        "funnel_7d must carry stage counts only, no medians: {funnel_7d}"
    );
}
