//! PRD-mcphost-invite-links
//! AC6 — Given six fresh sessions from one IP joining via one invite
//! within an hour, When each makes its first call, Then all six succeed
//! (per-code limiter, not per-IP) and a 21st within the hour is refused
//! with `invite_rate_limited`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn per_code_limiter_allows_many_joins_from_one_ip_then_caps_at_twenty_per_hour() {
    let server = TestServer::start().await;
    let (_a_ns, a_key) = signup(&server.base_url, "Inviter A").await;
    let a_client = McpClient::with_bearer(&server.base_url, &a_key);

    let created = extract_structured(
        &a_client
            .tools_call("host.invite.create", json!({"max_uses": 100}))
            .await
            .expect("invite create should succeed"),
    );
    let url = created["url"].as_str().unwrap().to_string();
    let url = url.trim_start_matches(&server.base_url).to_string();

    // Every join in this test comes from the same test process -- i.e.
    // the same source IP -- so succeeding past a per-IP signup cap (the
    // default is far below 20) is itself proof this path isn't using it.
    for n in 0..6 {
        let client = McpClient::new(&server.base_url).with_path(&url);
        client
            .tools_call("host.whoami", json!({}))
            .await
            .unwrap_or_else(|e| panic!("join {n} of 6 (same IP) must succeed: {e:?}"));
    }
    for n in 6..20 {
        let client = McpClient::new(&server.base_url).with_path(&url);
        client
            .tools_call("host.whoami", json!({}))
            .await
            .unwrap_or_else(|e| panic!("join {n} of 20 must succeed: {e:?}"));
    }

    let client21 = McpClient::new(&server.base_url).with_path(&url);
    let err = client21
        .tools_call("host.whoami", json!({}))
        .await
        .expect_err("the 21st join within the hour must be refused");
    assert_eq!(err.error_code, Some("invite_rate_limited".to_string()));
}
