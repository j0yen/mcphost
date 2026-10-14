//! PRD-mcphost-scheduled-job-receipts AC1 (P0) — Given a schedule fired 5
//! times (3 ok, then 2 failing) via `host.trigger.fire`, When
//! `host.schedule_receipts(trigger_id)` is called once, Then it returns 5
//! rows and `summary.streak_failed == 2`, `summary.ok == 3`,
//! `summary.next_fire_unix` set, while `host.runs.list(trigger="schedule")`
//! carries none of `streak_failed`, `last_ok_unix`, `next_fire_unix` (the
//! falsifier: if it does, receipts are redundant).

use crate::common;
use crate::sjrc;

use common::{McpClient, TestServer, extract_structured, signup_and_make_pro};
use serde_json::json;
use sjrc::{fire_until_receipts, publish_probe};

#[tokio::test]
async fn one_receipts_call_carries_the_streak_that_runs_list_cannot() {
    let server = TestServer::start_with_kinds(common::chain_kind_registry()).await;
    let (_ns, key, _id) = signup_and_make_pro(&server, "SJRC AC1 Tenant", "cus_sjrc1").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    publish_probe(&client).await;
    let set = extract_structured(
        &client
            .tools_call("host.trigger.set", json!({"tool": "probe", "schedule": "*/5 * * * *"}))
            .await
            .expect("trigger.set"),
    );
    let trigger_id = set["id"].as_str().expect("id").to_string();

    for n in 1..=3 {
        fire_until_receipts(&client, &trigger_id, n).await;
    }
    client
        .tools_call("host.tool_remove", json!({"name": "gone_dep"}))
        .await
        .expect("remove gone_dep");
    fire_until_receipts(&client, &trigger_id, 4).await;
    let got = fire_until_receipts(&client, &trigger_id, 5).await;

    assert_eq!(got["rows"].as_array().expect("rows").len(), 5, "{got:?}");
    let s = &got["summary"];
    assert_eq!(s["streak_failed"], json!(2), "{got:?}");
    assert_eq!(s["ok"], json!(3), "{got:?}");
    assert_eq!(s["failed"], json!(2), "{got:?}");
    assert!(s["next_fire_unix"].is_i64(), "next_fire_unix must be set: {got:?}");
    assert!(s["last_ok_unix"].is_i64(), "{got:?}");
    assert!(s["last_error"].is_string(), "{got:?}");

    // The falsifier: runs.list has no such aggregate fields anywhere.
    let listed = extract_structured(
        &client
            .tools_call("host.runs.list", json!({"trigger": "schedule"}))
            .await
            .expect("runs.list"),
    );
    let dump = listed.to_string();
    for field in ["streak_failed", "last_ok_unix", "next_fire_unix"] {
        assert!(!dump.contains(field), "runs.list unexpectedly carries {field}: {dump}");
    }
}
