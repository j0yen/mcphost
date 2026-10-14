//! PRD-mcphost-scheduled-job-receipts AC6 (P0) — Given a manual
//! `host.trigger.fire`, a scheduled fire, and a signed POST to an event
//! trigger, When each run ends, Then three receipts exist with `source`
//! manual, schedule and event respectively, and `host.trigger.get` on each
//! shows `last_receipt`.

use crate::common;
use crate::sjrc;

use common::{McpClient, TestServer, extract_structured, signup_and_make_pro};
use serde_json::{Value, json};
use sjrc::wait_receipts;

async fn publish_echo(client: &McpClient, name: &str) {
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": name, "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
}

async fn set_trigger(client: &McpClient, args: Value) -> Value {
    extract_structured(&client.tools_call("host.trigger.set", args).await.expect("trigger.set"))
}

async fn get_trigger(client: &McpClient, id: &str) -> Value {
    extract_structured(&client.tools_call("host.trigger.get", json!({"id": id})).await.expect("trigger.get"))
}

#[tokio::test]
async fn each_way_of_firing_leaves_one_receipt_with_its_own_source() {
    let server = TestServer::start().await;
    let (ns, key, _id) = signup_and_make_pro(&server, "SJRC AC6 Tenant", "cus_sjrc6").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    for tool in ["manual_tool", "sched_tool", "hook_tool"] {
        publish_echo(&client, tool).await;
    }
    let manual = set_trigger(&client, json!({"tool": "manual_tool", "schedule": "*/5 * * * *"})).await;
    let sched = set_trigger(&client, json!({"tool": "sched_tool", "schedule": "*/5 * * * *"})).await;
    let hook = set_trigger(
        &client,
        json!({
            "tool": "hook_tool",
            "kind": "event",
            "verify": {"scheme": "none", "allow_unverified": true},
        }),
    )
    .await;
    let (manual_id, sched_id, hook_id) = (
        manual["id"].as_str().expect("id").to_string(),
        sched["id"].as_str().expect("id").to_string(),
        hook["id"].as_str().expect("id").to_string(),
    );
    for id in [&manual_id, &sched_id, &hook_id] {
        let before = get_trigger(&client, id).await;
        assert_eq!(before["last_receipt"], Value::Null, "no receipt before the first run: {before:?}");
        assert_eq!(before["streak_failed"], json!(0), "{before:?}");
    }

    // Manual.
    client
        .tools_call("host.trigger.fire", json!({"id": manual_id}))
        .await
        .expect("fire");
    // Scheduled: force the trigger due and let the scheduler tick enqueue it.
    server
        .state
        .db
        .update_trigger_after_fire(sched_id.clone(), Some(mcphost::state::now_unix()), None, 0)
        .await
        .expect("force due");
    mcphost::triggers::tick_once(&server.state).await.expect("tick");
    // Event: a real POST to the trigger's public URL.
    let resp = reqwest::Client::new()
        .post(format!("{}/hooks/{}/hook_tool", server.base_url, ns))
        .body(json!({"n": 1}).to_string())
        .send()
        .await
        .expect("POST /hooks/...");
    assert_eq!(resp.status(), 202);

    for (id, source) in [(&manual_id, "manual"), (&sched_id, "schedule"), (&hook_id, "event")] {
        let got = wait_receipts(&client, id, 1).await;
        assert_eq!(got["rows"][0]["source"], json!(source), "{got:?}");
        assert_eq!(got["rows"][0]["status"], json!("done"), "{got:?}");
        let trigger = get_trigger(&client, id).await;
        assert_eq!(trigger["last_receipt"]["source"], json!(source), "{trigger:?}");
        assert_eq!(trigger["last_receipt"]["run_id"], got["rows"][0]["run_id"], "{trigger:?}");
    }
}
