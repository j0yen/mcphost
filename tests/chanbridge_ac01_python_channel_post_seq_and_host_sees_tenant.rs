//! PRD-mcphost-sandbox-channel-msg-bridge
//! AC1 (P0) — Given a published python tool whose source does `from
//! mcphost import channel; channel.post(cid, {"n": 1})` with `cid` a
//! channel the tenant opened, When called, Then the call result carries
//! the post's `seq` and `host.channel.read(cid)` returns the post with the
//! tenant as sender.

use crate::common;
use common::{TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn mcphost_channel_post_carries_seq_and_attributes_tenant() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Chanbridge AC1 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    // The tenant opens its own channel (a group it owns) -- the owner is
    // always an authorized member of a channel it opened, same as
    // `channel_ac01`'s own "O reads too" assertion.
    client
        .tools_call("host.group.create", json!({"name": "alerts"}))
        .await
        .expect("group.create");
    let opened = extract_structured(
        &client
            .tools_call("host.channel.open", json!({"group": "alerts"}))
            .await
            .expect("channel.open"),
    );
    let channel_id = opened["channel_id"].as_str().expect("channel_id").to_string();

    let spec = json!({
        "source": "from mcphost import channel\ndef main(args):\n    return channel.post(args[\"cid\"], {\"n\": 1})\n",
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "poster", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let qualified = format!("{ns}.poster");
    let result = extract_structured(
        &poll_until_ready(
            &client,
            &qualified,
            json!({"cid": channel_id}),
            Duration::from_secs(10),
        )
        .await
        .unwrap_or_else(|e| panic!("tool call must succeed: {} {}", e.code, e.message)),
    );
    assert_eq!(result["seq"], json!(1), "{result:?}");

    let read = extract_structured(
        &client
            .tools_call("host.channel.read", json!({"channel_id": channel_id}))
            .await
            .expect("host.channel.read"),
    );
    let posts = read["posts"].as_array().expect("posts array");
    assert_eq!(posts.len(), 1, "{read:?}");
    assert_eq!(posts[0]["seq"], json!(1), "{read:?}");
    assert_eq!(posts[0]["from_address"], json!(ns), "{read:?}");
}
