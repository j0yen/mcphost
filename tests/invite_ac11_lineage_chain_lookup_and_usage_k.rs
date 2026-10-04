//! PRD-mcphost-invite-links
//! AC11 — Given a chain A → B → C created through standing invites, When
//! `host.agent.lookup` is called for B and C and `host.usage` for A, Then
//! B shows `invited_by: A` and `invitees_count: 1`, C shows `invited_by:
//! B`, and A's `usage.invites.accepted_7d` is 1 with `k` computed as
//! specified.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

async fn standing_invite_path(server: &common::TestServer, client: &McpClient) -> String {
    let whoami = extract_structured(&client.tools_call("host.whoami", json!({})).await.expect("whoami"));
    let url = whoami["invite_url"].as_str().expect("invite_url present").to_string();
    url.trim_start_matches(&server.base_url).to_string()
}

#[tokio::test]
async fn lineage_chain_lookup_shows_invited_by_and_usage_reports_k() {
    let server = TestServer::start().await;
    let (a_ns, a_key) = signup(&server.base_url, "Inviter A").await;
    let a_client = McpClient::with_bearer(&server.base_url, &a_key);

    let a_invite_path = standing_invite_path(&server, &a_client).await;

    let b_session = McpClient::new(&server.base_url)
        .with_path(&a_invite_path)
        .with_session_continuity();
    b_session
        .tools_call("host.whoami", json!({}))
        .await
        .expect("B's first call (joining A's standing invite) must succeed");
    let b_invite_path = standing_invite_path(&server, &b_session).await;

    let c_session = McpClient::new(&server.base_url)
        .with_path(&b_invite_path)
        .with_session_continuity();
    c_session
        .tools_call("host.whoami", json!({}))
        .await
        .expect("C's first call (joining B's standing invite) must succeed");

    // Recover B's and C's namespaces for the lookup calls below.
    let b_ns = extract_structured(
        &b_session.tools_call("host.whoami", json!({})).await.expect("B whoami"),
    )["namespace"]
        .as_str()
        .unwrap()
        .to_string();
    let c_ns = extract_structured(
        &c_session.tools_call("host.whoami", json!({})).await.expect("C whoami"),
    )["namespace"]
        .as_str()
        .unwrap()
        .to_string();

    let b_lookup = extract_structured(
        &a_client
            .tools_call("host.agent.lookup", json!({"address": b_ns}))
            .await
            .expect("lookup B should succeed"),
    );
    assert_eq!(b_lookup["invited_by"], json!(a_ns));
    assert_eq!(b_lookup["invitees_count"], json!(1));

    let c_lookup = extract_structured(
        &a_client
            .tools_call("host.agent.lookup", json!({"address": c_ns}))
            .await
            .expect("lookup C should succeed"),
    );
    assert_eq!(c_lookup["invited_by"], json!(b_ns));

    let usage = extract_structured(
        &a_client.tools_call("host.usage", json!({})).await.expect("A's usage should succeed"),
    );
    assert_eq!(usage["invites"]["accepted_7d"], json!(1));
    // A and B are the only two distinct tenants who accepted an invite of
    // their own in this isolated server, so k = accepted_7d / 2.
    assert_eq!(usage["invites"]["k"].as_f64().unwrap(), 0.5);
}
