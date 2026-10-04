//! PRD-mcphost-invite-links
//! AC4 — Given an invite with `max_uses: 2` and two concurrent first calls
//! from two fresh sessions plus a third, When they race, Then exactly two
//! tenants are created and the third receives `invite_invalid`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn race_for_the_last_slot_yields_exactly_max_uses_successes() {
    let server = TestServer::start().await;
    let (a_ns, a_key) = signup(&server.base_url, "Inviter A").await;
    let a_client = McpClient::with_bearer(&server.base_url, &a_key);

    a_client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "mytool",
                "kind": "echo",
                "spec": {"schema": {"type": "object", "properties": {}, "required": []}},
            }),
        )
        .await
        .expect("publish should succeed");
    let created = a_client
        .tools_call("host.invite.create", json!({"max_uses": 2, "share": ["mytool"]}))
        .await
        .expect("invite create should succeed");
    let url = extract_structured(&created)["url"].as_str().unwrap().to_string();
    let path = url.trim_start_matches(&server.base_url).to_string();

    let call = |n: u32| {
        let url = path.clone();
        let base_url = server.base_url.clone();
        let a_ns = a_ns.clone();
        async move {
            let client = McpClient::new(&base_url).with_path(&url);
            let _ = n;
            client.tools_call("host.tool.call", json!({"name": format!("{a_ns}.mytool")})).await
        }
    };

    let (r1, r2, r3) = tokio::join!(call(1), call(2), call(3));

    let results = [r1, r2, r3];
    let successes = results.iter().filter(|r| r.is_ok()).count();
    let failures: Vec<_> = results.iter().filter_map(|r| r.as_ref().err()).collect();

    assert_eq!(successes, 2, "exactly max_uses (2) calls must succeed: {results:?}");
    assert_eq!(failures.len(), 1, "exactly one call must fail: {results:?}");
    assert_eq!(
        failures[0].error_code,
        Some("invite_invalid".to_string()),
        "the losing call must receive invite_invalid: {:?}",
        failures[0]
    );

    let listed = extract_structured(
        &a_client
            .tools_call("host.invite.list", json!({}))
            .await
            .expect("invite list should succeed"),
    );
    let entry = listed["invites"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["kind"] == json!("standard"))
        .expect("the standard invite must be listed");
    assert_eq!(entry["uses"], json!(2));
    assert_eq!(
        entry["invitees"].as_array().unwrap().len(),
        2,
        "exactly two invitee tenants must have been created"
    );
}
