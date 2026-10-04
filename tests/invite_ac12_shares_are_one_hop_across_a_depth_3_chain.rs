//! PRD-mcphost-invite-links
//! AC12 — Given A shared tool `t` to B via an invite with `share=["t"]`
//! and B invited C through B's standing invite, When C calls `<A ns>.t`,
//! Then the call is refused as not shared, and When B calls
//! `host.tool_share` for `t` to C, Then it is refused because B does not
//! own `t`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn shares_are_one_hop_across_a_depth_3_chain() {
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
    let created = extract_structured(
        &a_client
            .tools_call("host.invite.create", json!({"share": ["mytool"]}))
            .await
            .expect("invite create should succeed"),
    );
    let a_invite_path = created["url"]
        .as_str()
        .unwrap()
        .trim_start_matches(&server.base_url)
        .to_string();

    // B joins A's invite -- B now has access to A's shared tool.
    let b_session = McpClient::new(&server.base_url)
        .with_path(&a_invite_path)
        .with_session_continuity();
    b_session
        .tools_call("host.tool.call", json!({"name": format!("{a_ns}.mytool")}))
        .await
        .expect("B's share from A must work");

    // B's own standing invite shares nothing by default.
    let b_whoami = extract_structured(&b_session.tools_call("host.whoami", json!({})).await.expect("B whoami"));
    let b_invite_url = b_whoami["invite_url"].as_str().expect("B's invite_url").to_string();
    let b_invite_path = b_invite_url.trim_start_matches(&server.base_url).to_string();

    // C joins B's standing invite.
    let c_session = McpClient::new(&server.base_url)
        .with_path(&b_invite_path)
        .with_session_continuity();
    c_session
        .tools_call("host.whoami", json!({}))
        .await
        .expect("C's first call (joining B's standing invite) must succeed");

    // C may not reach A's tool -- shares are one hop, not transitive.
    let err = c_session
        .tools_call("host.tool.call", json!({"name": format!("{a_ns}.mytool")}))
        .await
        .expect_err("C must not reach A's tool through B");
    assert_eq!(err.error_code, Some("tool_not_found".to_string()));

    // B cannot re-share a tool it doesn't own, to C or anyone else.
    let err = b_session
        .tools_call("host.tool_share", json!({"name": "mytool", "visibility": "public"}))
        .await
        .expect_err("B does not own mytool and must not be able to share it");
    assert_eq!(err.error_code, Some("tool_not_found".to_string()));
}
