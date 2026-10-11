//! PRD-mcphost-data-quota-in-every-response
//! AC5 -- Given a tenant at `state_bytes_max`, When `host.state.set` runs,
//! Then `quota_exceeded` carries `data.quota.state_bytes_max.remaining = 0`
//! and the other ceilings with their real `remaining`; Given a tenant whose
//! plan name is missing from the catalog, Then `data.quota` is omitted and
//! the call still succeeds.
//!
//! The legacy refusal field `data.quota` is the ceiling's NAME (pinned by
//! `state_ac06`), so on that refusal the object rides as `data.quota_room`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn state_refusal_carries_the_room_left_on_every_state_ceiling() {
    let server = TestServer::start_with_plans(|catalog| {
        for p in &mut catalog.plans {
            if p.name == "free" {
                p.state_bytes_max = 1000;
            }
        }
    })
    .await;
    let (_ns, key) = signup(&server.base_url, "Quota").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    // 998 chars + the two JSON quotes = exactly 1000 stored bytes.
    client
        .tools_call("host.state.set", json!({"key": "fill", "value": "a".repeat(998)}))
        .await
        .expect("fill to the ceiling");

    let err = client
        .tools_call("host.state.set", json!({"key": "over", "value": "b"}))
        .await
        .expect_err("past the ceiling must refuse");
    assert_eq!(err.error_code.as_deref(), Some("state_quota_exceeded"));
    assert_eq!(err.data["quota"], json!("state_bytes_max"), "legacy field unchanged");
    let room = &err.data["quota_room"];
    assert_eq!(room["state_bytes_max"], json!({"used": 1000, "limit": 1000, "remaining": 0}), "{:?}", err.data);
    let free = server.state.plans.get("free").expect("free");
    assert_eq!(
        room["state_rows_max"],
        json!({"used": 0, "limit": free.state_rows_max, "remaining": free.state_rows_max}),
        "the other state ceilings report their real room: {:?}",
        err.data
    );
}

#[tokio::test]
async fn a_plan_missing_from_the_catalog_omits_data_quota_and_the_call_succeeds() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Quota").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let ok = extract_structured(&client.tools_call("host.state.list", json!({})).await.expect("list"));
    assert!(ok["data"]["quota"].is_object(), "control: a catalogued plan carries it: {ok:?}");

    let db = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("open db");
    db.execute("UPDATE tenants SET plan = 'ghost'", []).expect("rename plan");

    let after = extract_structured(&client.tools_call("host.state.list", json!({})).await.expect("still succeeds"));
    assert!(after.get("data").is_none_or(|d| d.get("quota").is_none()), "{after:?}");
}
