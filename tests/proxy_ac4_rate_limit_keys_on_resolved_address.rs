//! PRD-mcphost-client-ip-behind-proxy
//! AC4 — Given `MCPHOST_SIGNUP_RATE_LIMIT_PER_HOUR=2` and two signups from
//! loopback with `X-Forwarded-For: 203.0.113.7` followed by one from
//! loopback with `X-Forwarded-For: 203.0.113.8`, When they run in one hour,
//! Then the third succeeds and a fourth from `203.0.113.7` is
//! rate-limited.

use crate::common;
use common::{McpClient, TestServer};
use serde_json::json;

#[tokio::test]
async fn rate_limiter_keys_on_resolved_forwarded_address_not_shared_peer() {
    let server = TestServer::start_with_signup_rate_limit(2).await;
    let client = McpClient::new(&server.base_url);

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

    // The "third" signup overall, but the first from .8's own bucket -- it
    // must succeed even though the shared loopback peer already recorded
    // two signups, proving the limiter keys on the resolved address rather
    // than the one TCP peer every proxied request shares.
    client
        .tools_call_with_header(
            "signup",
            json!({"name": "Agent8"}),
            ("x-forwarded-for", "203.0.113.8"),
        )
        .await
        .unwrap_or_else(|e| {
            panic!("a signup from a different forwarded address must not be limited by .7's bucket: {e:?}")
        });

    let err = client
        .tools_call_with_header(
            "signup",
            json!({"name": "Agent7-third"}),
            ("x-forwarded-for", "203.0.113.7"),
        )
        .await
        .expect_err("a third signup from .7 within the hour must be rate limited");
    assert_eq!(err.error_code.as_deref(), Some("rate_limited"));
}
