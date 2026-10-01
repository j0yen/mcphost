//! PRD-mcphost-invite-links
//! AC4 (P0) — Given an invite with `max_uses: 2` and two concurrent
//! first calls from two fresh sessions plus a third, When they race,
//! Then exactly two tenants are created and the third receives
//! `invite_invalid`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn race_for_the_last_slot_admits_exactly_max_uses() {
    let server = TestServer::start().await;
    let (ns_a, key_a) = signup(&server.base_url, "AC4 Inviter").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);

    client_a
        .tools_call(
            "host.tool_publish",
            json!({"name": "tt", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish t");
    let created = client_a
        .tools_call("host.invite.create", json!({"share": ["tt"], "max_uses": 2}))
        .await
        .expect("host.invite.create");
    let url = extract_structured(&created)["url"].as_str().expect("url").to_string();
    let path = url.trim_start_matches(&server.base_url).to_string();

    let make_call = |path: String| {
        let base_url = server.base_url.clone();
        let ns_a = ns_a.clone();
        async move {
            let session = McpClient::new(&base_url).with_path(&path).with_session_continuity();
            session
                .tools_call("host.tool_call", json!({"name": format!("{ns_a}.tt"), "args": {}}))
                .await
        }
    };

    let (r1, r2, r3) = tokio::join!(
        make_call(path.clone()),
        make_call(path.clone()),
        make_call(path.clone()),
    );
    let results = [r1, r2, r3];
    let successes = results.iter().filter(|r| r.is_ok()).count();
    let failures: Vec<_> = results.iter().filter_map(|r| r.as_ref().err()).collect();

    assert_eq!(successes, 2, "exactly max_uses tenants must be created: {results:?}");
    assert_eq!(failures.len(), 1, "exactly one caller must lose the race: {results:?}");
    assert_eq!(
        failures[0].error_code.as_deref(),
        Some("invite_invalid"),
        "{:?}",
        failures[0]
    );
}
