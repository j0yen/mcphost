//! PRD-mcphost-scheduled-job-receipts AC4 (P0) — Given `host.trigger.pause`,
//! When the scheduler ticks three times, Then no receipt and no message is
//! produced; Given `host.trigger.resume`, Then the next fire's receipt exists
//! and `streak_failed` restarts at 0.

use crate::common;
use crate::sjrc;

use common::{McpClient, TestServer, extract_structured, signup_and_make_pro};
use serde_json::json;
use sjrc::{break_dep, fire_until_receipts, host_messages, publish_probe, restore_dep, settle, wait_receipts};

async fn force_due_and_tick(server: &TestServer, trigger_id: &str) {
    let now = mcphost::state::now_unix();
    server
        .state
        .db
        .update_trigger_after_fire(trigger_id.to_string(), Some(now), None, 0)
        .await
        .expect("force due");
    mcphost::triggers::tick_once(&server.state).await.expect("tick");
}

#[tokio::test]
async fn paused_schedule_leaves_no_trace_and_resume_restarts_the_streak() {
    let server = TestServer::start_with_kinds(common::chain_kind_registry()).await;
    let (_ns, key, _id) = signup_and_make_pro(&server, "SJRC AC4 Tenant", "cus_sjrc4").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    publish_probe(&client).await;
    let set = extract_structured(
        &client
            .tools_call("host.trigger.set", json!({"tool": "probe", "schedule": "*/5 * * * *"}))
            .await
            .expect("trigger.set"),
    );
    let trigger_id = set["id"].as_str().expect("id").to_string();
    break_dep(&client).await;

    // Two failures before the pause: one short of the alert.
    fire_until_receipts(&client, &trigger_id, 1).await;
    fire_until_receipts(&client, &trigger_id, 2).await;

    client
        .tools_call("host.trigger.pause", json!({"id": trigger_id}))
        .await
        .expect("pause");
    for _ in 0..3 {
        force_due_and_tick(&server, &trigger_id).await;
    }
    settle().await;
    let got = wait_receipts(&client, &trigger_id, 2).await;
    assert_eq!(got["summary"]["runs"], json!(2), "a paused trigger must add no receipt: {got:?}");
    assert!(host_messages(&client).await.is_empty(), "a paused trigger must send no message");

    // Resumed: the next scheduled fire has a receipt, and the two failures
    // from before the pause do not count towards this fire's streak.
    client
        .tools_call("host.trigger.resume", json!({"id": trigger_id}))
        .await
        .expect("resume");
    force_due_and_tick(&server, &trigger_id).await;
    let got = wait_receipts(&client, &trigger_id, 3).await;
    assert_eq!(got["rows"][0]["source"], json!("schedule"), "{got:?}");
    assert_eq!(got["summary"]["streak_failed"], json!(1), "{got:?}");
    settle().await;
    assert!(host_messages(&client).await.is_empty(), "streak restarted: no alert at 1 failure");

    // A success after the resume leaves the streak at 0.
    restore_dep(&client).await;
    let got = fire_until_receipts(&client, &trigger_id, 4).await;
    assert_eq!(got["summary"]["streak_failed"], json!(0), "{got:?}");
}
