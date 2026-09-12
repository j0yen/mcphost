//! PRD-mcphost-client-ip-behind-proxy
//! AC6 — Given the authenticated `/healthz`, When read after AC4's
//! signups, Then `distinct_source_ips_24h` is 2.

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
async fn distinct_source_ips_24h_counts_resolved_addresses() {
    let server = TestServer::start_with_signup_rate_limit(2).await;
    let client = McpClient::new(&server.base_url);

    // AC4's exact sequence: two signups from .7 (fills its bucket), one
    // from .8 (its own bucket), then a rate-limited fourth from .7 that
    // must not create a third distinct source.
    for i in 0..2 {
        client
            .tools_call_with_header(
                "signup",
                json!({"name": format!("Agent7-{i}")}),
                ("x-forwarded-for", "203.0.113.7"),
            )
            .await
            .unwrap_or_else(|e| panic!("signup {i} from .7 should succeed: {e:?}"));
    }
    client
        .tools_call_with_header(
            "signup",
            json!({"name": "Agent8"}),
            ("x-forwarded-for", "203.0.113.8"),
        )
        .await
        .unwrap_or_else(|e| panic!("signup from .8 should succeed: {e:?}"));
    let _ = client
        .tools_call_with_header(
            "signup",
            json!({"name": "Agent7-third"}),
            ("x-forwarded-for", "203.0.113.7"),
        )
        .await; // expected to be rate-limited; AC4 covers that assertion

    let body = healthz(&server.base_url).await;
    assert_eq!(
        body["distinct_source_ips_24h"].as_i64(),
        Some(2),
        "distinct_source_ips_24h must count .7 and .8, not the shared loopback peer: {body:?}"
    );
}
