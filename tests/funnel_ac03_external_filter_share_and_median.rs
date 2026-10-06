//! PRD-mcphost-activation-funnel
//! AC3 (P0) — Given 10 external and 50 synthetic signups in the window
//! with 4 external publishes, When `admin.funnel {days: 7, source_class:
//! "external"}` is read, Then `signups = 10`, the `first_publish` stage
//! shows `count = 4, share = 0.4` and a median in minutes; the synthetic
//! rows are absent.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn external_filter_reports_signups_share_and_median_without_synthetic_rows() {
    // 60 signups from this one process (10 external, via the fake-IP
    // business-logic seam, plus 50 loopback-sourced synthetic ones) would
    // otherwise trip the per-source signup rate limiter well before AC3's
    // own scenario is even set up.
    let server = TestServer::start_with_signup_rate_limit(100).await;

    // 10 external tenants (non-loopback source IP, via the business-logic
    // seam attrib_ac4_external_signup_counts_real.rs already established
    // for this exact reason: the test server itself is always loopback).
    let mut external = Vec::new();
    for i in 0..10 {
        let signed_up = mcphost::control::signup(
            &server.state,
            &json!({"name": format!("AC3 External {i}")}),
            &format!("8.8.{}.1", i + 1),
            mcphost::control::SignupAttribution::default(),
        )
        .await
        .expect("external signup");
        let namespace = signed_up["tenant"].as_str().expect("namespace").to_string();
        let key = signed_up["key"].as_str().expect("key").to_string();
        let tenant = server
            .state
            .db
            .find_tenant_by_namespace(namespace)
            .await
            .expect("query")
            .expect("tenant exists");
        assert_eq!(tenant.source_class.as_deref(), Some("external"), "{tenant:?}");
        external.push((tenant.id, key));
    }

    // 50 synthetic (loopback) signups, in the same window -- must be
    // excluded entirely from the `source_class: "external"` read.
    for i in 0..50 {
        common::signup(&server.base_url, &format!("AC3 Synthetic {i}")).await;
    }

    // 4 of the 10 external tenants publish, at four distinct gaps from
    // signup (1, 2, 5, 12 minutes) -- sorted [1, 2, 5, 12], median = (2 +
    // 5) / 2 = 3.5 minutes.
    let gaps_minutes: [i64; 4] = [1, 2, 5, 12];
    for (idx, gap) in gaps_minutes.iter().enumerate() {
        let (tenant_id, key) = &external[idx];
        let client = McpClient::with_bearer(&server.base_url, key);
        client
            .tools_call(
                "host.tool_publish",
                json!({
                    "name": "funnel_ac03_tool",
                    "kind": "echo",
                    "spec": {"schema": {"type": "object"}},
                }),
            )
            .await
            .expect("publish");
        let after_publish = server
            .state
            .db
            .find_tenant_by_id(*tenant_id)
            .await
            .expect("query")
            .expect("tenant exists");
        let first_publish_unix = after_publish.first_publish_unix.expect("first_publish_unix set");
        // Back-date `created_unix` so the elapsed gap is exactly `gap`
        // minutes, regardless of how fast this test actually ran.
        server
            .state
            .db
            .set_tenant_stamp_for_test(*tenant_id, "created_unix", first_publish_unix - gap * 60)
            .await
            .expect("backdate created_unix");
    }

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let funnel = extract_structured(
        &admin
            .tools_call("admin.funnel", json!({"days": 7, "source_class": "external"}))
            .await
            .expect("admin.funnel"),
    );

    assert_eq!(funnel["signups"], json!(10), "{funnel}");
    let stages = funnel["stages"].as_array().expect("stages array");
    let first_publish = stages
        .iter()
        .find(|s| s["name"] == json!("first_publish"))
        .expect("first_publish stage present");
    assert_eq!(first_publish["count"], json!(4), "{funnel}");
    assert_eq!(first_publish["share"].as_f64(), Some(0.4), "{funnel}");
    assert_eq!(first_publish["median_minutes"].as_f64(), Some(3.5), "{funnel}");
}
