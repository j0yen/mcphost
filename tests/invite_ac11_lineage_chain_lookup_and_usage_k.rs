//! PRD-mcphost-invite-links
//! AC11 (P0) — Given a chain A → B → C created through standing
//! invites, When `host.agent.lookup` is called for B and C and
//! `host.usage` for A, Then B shows `invited_by: A` and
//! `invitees_count: 1`, C shows `invited_by: B`, and A's
//! `usage.invites.accepted_7d` is 1 with `k` computed as specified.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

async fn standing_invite_path(server_base: &str, client: &McpClient) -> String {
    let whoami = client.tools_call("host.whoami", json!({})).await.expect("host.whoami");
    let url = extract_structured(&whoami)["invite_url"]
        .as_str()
        .expect("invite_url")
        .to_string();
    url.trim_start_matches(server_base).to_string()
}

#[tokio::test]
async fn lineage_chain_lookup_and_usage_k() {
    let server = TestServer::start().await;
    let (ns_a, key_a) = signup(&server.base_url, "AC11 A").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    let path_a = standing_invite_path(&server.base_url, &client_a).await;

    // B joins through A's standing invite.
    let client_b = McpClient::new(&server.base_url)
        .with_path(&path_a)
        .with_session_continuity();
    client_b
        .tools_call("host.whoami", json!({}))
        .await
        .expect("B joins via A's standing invite");
    let whoami_b = client_b.tools_call("host.whoami", json!({})).await.expect("host.whoami B");
    let whoami_b = extract_structured(&whoami_b);
    let ns_b = whoami_b["tenant"].as_str().expect("B's namespace").to_string();
    assert_eq!(whoami_b["invited_by"], json!(ns_a), "{whoami_b}");

    let path_b = standing_invite_path(&server.base_url, &client_b).await;

    // C joins through B's standing invite.
    let client_c = McpClient::new(&server.base_url)
        .with_path(&path_b)
        .with_session_continuity();
    client_c
        .tools_call("host.whoami", json!({}))
        .await
        .expect("C joins via B's standing invite");
    let whoami_c = client_c.tools_call("host.whoami", json!({})).await.expect("host.whoami C");
    let whoami_c = extract_structured(&whoami_c);
    let ns_c = whoami_c["tenant"].as_str().expect("C's namespace").to_string();
    assert_eq!(whoami_c["invited_by"], json!(ns_b), "{whoami_c}");

    let lookup_b = client_a
        .tools_call("host.agent.lookup", json!({"address": ns_b}))
        .await
        .expect("host.agent.lookup B");
    let lookup_b = extract_structured(&lookup_b);
    assert_eq!(lookup_b["invited_by"], json!(ns_a), "{lookup_b}");
    assert_eq!(lookup_b["invitees_count"], json!(1), "{lookup_b}");

    let lookup_c = client_a
        .tools_call("host.agent.lookup", json!({"address": ns_c}))
        .await
        .expect("host.agent.lookup C");
    let lookup_c = extract_structured(&lookup_c);
    assert_eq!(lookup_c["invited_by"], json!(ns_b), "{lookup_c}");

    let usage_a = client_a.tools_call("host.usage", json!({})).await.expect("host.usage A");
    let usage_a = extract_structured(&usage_a);
    assert_eq!(usage_a["invites"]["accepted_7d"], json!(1), "{usage_a}");
    assert_eq!(usage_a["invites"]["k"], json!(1.0), "{usage_a}");
}
