//! PRD-mcphost-scheduled-job-receipts AC2 (P0) — Given the third
//! consecutive failure, When the run ends, Then exactly one message from
//! `@host` with subject `schedule <tool> failed 3x in a row` and
//! `data.trigger_id` is in `host.msg.inbox`; the fourth failure sends
//! nothing; a success then two more failures send nothing; a success then
//! three failures send one more.

use crate::common;
use crate::sjrc;

use common::{McpClient, TestServer, extract_structured, signup_and_make_pro};
use serde_json::json;
use sjrc::{break_dep, fire_until_receipts, host_messages, publish_probe, restore_dep, settle};

#[tokio::test]
async fn one_message_per_streak_of_three_deduplicated_until_a_success() {
    let server = TestServer::start_with_kinds(common::chain_kind_registry()).await;
    let (_ns, key, _id) = signup_and_make_pro(&server, "SJRC AC2 Tenant", "cus_sjrc2").await;
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

    let mut n = 0;
    for _ in 0..2 {
        n += 1;
        fire_until_receipts(&client, &trigger_id, n).await;
    }
    settle().await;
    assert!(host_messages(&client).await.is_empty(), "two failures must not alert");

    n += 1;
    fire_until_receipts(&client, &trigger_id, n).await;
    settle().await;
    let msgs = host_messages(&client).await;
    assert_eq!(msgs.len(), 1, "{msgs:?}");
    assert_eq!(msgs[0]["body"], json!("schedule probe failed 3x in a row"));
    assert_eq!(msgs[0]["data"]["trigger_id"], json!(trigger_id));
    assert_eq!(msgs[0]["data"]["tool"], json!("probe"));
    assert!(msgs[0]["data"]["last_error"].is_string(), "{msgs:?}");
    assert!(msgs[0]["data"]["receipts_call"].is_string(), "{msgs:?}");

    // The fourth failure sends nothing.
    n += 1;
    fire_until_receipts(&client, &trigger_id, n).await;
    settle().await;
    assert_eq!(host_messages(&client).await.len(), 1, "fourth failure must not alert again");

    // A success, then two failures: still nothing new.
    restore_dep(&client).await;
    n += 1;
    fire_until_receipts(&client, &trigger_id, n).await;
    break_dep(&client).await;
    for _ in 0..2 {
        n += 1;
        fire_until_receipts(&client, &trigger_id, n).await;
    }
    settle().await;
    assert_eq!(host_messages(&client).await.len(), 1, "two failures after a success must not alert");

    // A third failure after that success: one more message.
    n += 1;
    fire_until_receipts(&client, &trigger_id, n).await;
    settle().await;
    assert_eq!(host_messages(&client).await.len(), 2, "success then three failures alert once more");
}
