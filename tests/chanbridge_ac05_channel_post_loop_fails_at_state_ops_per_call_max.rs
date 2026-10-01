//! PRD-mcphost-sandbox-channel-msg-bridge
//! AC5 (P0) — Given a tool that loops `channel.post` 1000 times, When
//! called, Then the call fails at `state_ops_per_call_max` with the shared
//! sidecar error and at most that many posts exist.

use crate::common;
use common::{TestServer, extract_structured, poll_until_ready, python_kind_registry, signup_and_make_pro};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn mcphost_channel_post_loop_stops_at_state_ops_per_call_max() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    // Pro, not free: `channel_posts_per_hour` (free 120, pro 2,000) would
    // otherwise trip before `state_ops_per_call_max` (200 on both plans)
    // does, masking the sidecar-ops cap this AC is actually about.
    let (ns, key, _tenant_id) =
        signup_and_make_pro(&server, "Chanbridge AC5 Tenant", "cus_chanbridge_ac5").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let state_ops_per_call_max = server.state.plans.get("pro").expect("pro plan").state_ops_per_call_max;

    client
        .tools_call("host.group.create", json!({"name": "flood"}))
        .await
        .expect("group.create");
    let opened = extract_structured(
        &client
            .tools_call("host.channel.open", json!({"group": "flood"}))
            .await
            .expect("channel.open"),
    );
    let channel_id = opened["channel_id"].as_str().expect("channel_id").to_string();

    let spec = json!({
        "source": "from mcphost import channel\ndef main(args):\n    posted = 0\n    try:\n        for _ in range(1000):\n            channel.post(args[\"cid\"], {\"n\": posted})\n            posted += 1\n    except channel.BridgeError as e:\n        return {\"posted\": posted, \"code\": e.code, \"data\": e.data}\n    return {\"posted\": posted, \"code\": None}\n",
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "flooder", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let qualified = format!("{ns}.flooder");
    let result = extract_structured(
        &poll_until_ready(&client, &qualified, json!({"cid": channel_id}), Duration::from_secs(20))
            .await
            .unwrap_or_else(|e| panic!("tool call must succeed: {} {}", e.code, e.message)),
    );
    assert_eq!(result["code"], json!("state_quota_exceeded"), "{result:?}");
    assert_eq!(result["data"]["quota"], json!("state_ops_per_call_max"), "{result:?}");
    assert_eq!(result["posted"], json!(state_ops_per_call_max), "{result:?}");

    // `host.channel.read`'s own `limit` argument clamps to 100 -- queried
    // directly against the DB (same "bypass the business-logic clamp for an
    // exact count" convention `state_ac06`'s own `state_bytes_used` call
    // uses) so a bug that let the loop run past the cap wouldn't be masked
    // by the read API's own pagination ceiling.
    let posts = server
        .state
        .db
        .channel_posts_after(channel_id.clone(), 0, 1_000)
        .await
        .expect("channel_posts_after");
    assert!(
        posts.len() as i64 <= state_ops_per_call_max,
        "at most {state_ops_per_call_max} posts may exist, got {}",
        posts.len()
    );
}
