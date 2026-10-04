//! PRD-mcphost-invite-links
//! AC7 — Given a free-plan inviter with 3 live invites, When
//! `host.invite.create` is called again, Then the plan error names
//! `invites_max: 3`; after `revoke` of one, creation succeeds.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn free_plan_caps_at_three_live_invites_and_revoke_frees_a_slot() {
    let server = TestServer::start().await;
    let (_a_ns, a_key) = signup(&server.base_url, "Inviter A").await;
    let a_client = McpClient::with_bearer(&server.base_url, &a_key);

    let mut codes = Vec::new();
    for _ in 0..3 {
        let created = extract_structured(
            &a_client
                .tools_call("host.invite.create", json!({}))
                .await
                .expect("the first three invites must succeed on the free plan"),
        );
        codes.push(created["code"].as_str().unwrap().to_string());
    }

    let err = a_client
        .tools_call("host.invite.create", json!({}))
        .await
        .expect_err("a fourth live invite must be refused on the free plan");
    assert_eq!(err.error_code, Some("invites_quota_exceeded".to_string()));
    assert_eq!(err.data["invites_max"], json!(3));

    a_client
        .tools_call("host.invite.revoke", json!({"code": codes[0]}))
        .await
        .expect("revoking one of the three must succeed");

    a_client
        .tools_call("host.invite.create", json!({}))
        .await
        .expect("creation must succeed again once a slot is freed by revoke");
}
