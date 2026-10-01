//! PRD-mcphost-invite-links
//! AC12 (P0) — Given A shared tool `t` to B via an invite with
//! `share=["tt"]` and B invited C through B's standing invite, When C
//! calls `<A ns>.t`, Then the call is refused as not shared, and When B
//! calls `host.tool_share` for `t` to C, Then it is refused because B
//! does not own `t`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn shares_are_one_hop_depth_three_chain() {
    let server = TestServer::start().await;
    let (ns_a, key_a) = signup(&server.base_url, "AC12 A").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);

    client_a
        .tools_call(
            "host.tool_publish",
            json!({"name": "tt", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish t");
    let created = client_a
        .tools_call("host.invite.create", json!({"share": ["tt"]}))
        .await
        .expect("host.invite.create");
    let url_a = extract_structured(&created)["url"].as_str().expect("url").to_string();
    let path_a = url_a.trim_start_matches(&server.base_url).to_string();

    // B joins through A's invite, gaining `t`.
    let client_b = McpClient::new(&server.base_url)
        .with_path(&path_a)
        .with_session_continuity();
    client_b
        .tools_call("host.tool_call", json!({"name": format!("{ns_a}.tt"), "args": {}}))
        .await
        .expect("B joins via A's invite and can reach t");
    let whoami_b = client_b.tools_call("host.whoami", json!({})).await.expect("host.whoami B");
    let standing_url_b = extract_structured(&whoami_b)["invite_url"]
        .as_str()
        .expect("B's standing invite_url")
        .to_string();
    let path_b = standing_url_b.trim_start_matches(&server.base_url).to_string();

    // C joins through B's OWN standing invite (default share: []).
    let client_c = McpClient::new(&server.base_url)
        .with_path(&path_b)
        .with_session_continuity();
    client_c
        .tools_call("host.whoami", json!({}))
        .await
        .expect("C joins via B's standing invite");

    // C cannot reach A's tool `t` -- one hop only.
    let err = client_c
        .tools_call("host.tool_call", json!({"name": format!("{ns_a}.tt"), "args": {}}))
        .await
        .expect_err("C must not be able to reach A's tool t through B");
    assert_eq!(err.error_code.as_deref(), Some("tool_not_found"), "{err:?}");

    // B cannot re-share a tool it does not own.
    // `visibility: "public"` skips the group-existence check entirely, so
    // this exercises exactly the ownership check (`Db::get_tool` scoped to
    // B's own tenant id) requirement 13 names, not a `group_not_found`
    // refusal from an unrelated group argument.
    let share_err = client_b
        .tools_call("host.tool_share", json!({"name": "tt", "visibility": "public"}))
        .await
        .expect_err("B does not own t and must not be able to share it");
    assert_eq!(share_err.error_code.as_deref(), Some("tool_not_found"), "{share_err:?}");
}
