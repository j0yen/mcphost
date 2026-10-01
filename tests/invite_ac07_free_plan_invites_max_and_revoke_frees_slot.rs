//! PRD-mcphost-invite-links
//! AC7 (P0) — Given a free-plan inviter with 3 live invites, When
//! `host.invite.create` is called again, Then the plan error names
//! `invites_max: 3`; after `revoke` of one, creation succeeds.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn fourth_create_refused_until_one_is_revoked() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC7 Inviter").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let mut codes = Vec::new();
    for _ in 0..3 {
        let created = client
            .tools_call("host.invite.create", json!({}))
            .await
            .expect("create a live invite");
        codes.push(extract_structured(&created)["code"].as_str().expect("code").to_string());
    }

    let err = client
        .tools_call("host.invite.create", json!({}))
        .await
        .expect_err("the 4th live invite must be refused on the free plan");
    assert_eq!(err.error_code.as_deref(), Some("invites_max"));
    assert_eq!(err.data["invites_max"], json!(3), "{:?}", err.data);

    client
        .tools_call("host.invite.revoke", json!({"code": codes[0].clone()}))
        .await
        .expect("revoke one invite");

    client
        .tools_call("host.invite.create", json!({}))
        .await
        .expect("create succeeds again once a slot is free");
}
