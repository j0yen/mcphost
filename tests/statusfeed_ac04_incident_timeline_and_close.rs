//! PRD-mcphost-status-feed AC4 (P0): given an open incident, when
//! `admin.incident.update {id, message}` then `admin.incident.close {id,
//! message}` run, then the timeline has three entries in order and the
//! incident moves to `incidents_recent_30d`.

use crate::common;

use common::{ADMIN_KEY, McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn update_then_close_builds_ordered_timeline() {
    let server = TestServer::start().await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    let opened = admin
        .tools_call(
            "admin.incident.open",
            json!({"title": "billing webhook flaky", "impact": "minor", "components": ["billing"]}),
        )
        .await
        .expect("admin.incident.open");
    let id = extract_structured(&opened)["id"].as_i64().expect("incident id");

    admin
        .tools_call("admin.incident.update", json!({"id": id, "message": "still investigating"}))
        .await
        .expect("admin.incident.update");
    admin
        .tools_call("admin.incident.close", json!({"id": id, "message": "resolved: retried webhook secret"}))
        .await
        .expect("admin.incident.close");

    let body = mcphost::statusfeed::status_json(&server.state)
        .await
        .expect("status_json");
    let open_ids: Vec<i64> = body["incidents_open"]
        .as_array()
        .expect("incidents_open array")
        .iter()
        .map(|i| i["id"].as_i64().expect("id"))
        .collect();
    assert!(!open_ids.contains(&id), "incident {id} should no longer be open: {open_ids:?}");

    let recent = body["incidents_recent_30d"].as_array().expect("incidents_recent_30d array");
    let incident = recent
        .iter()
        .find(|i| i["id"] == json!(id))
        .unwrap_or_else(|| panic!("incident {id} not in incidents_recent_30d: {recent:?}"));

    let timeline = incident["timeline"].as_array().expect("timeline array");
    assert_eq!(timeline.len(), 3, "timeline: {timeline:?}");
    assert_eq!(timeline[0]["type"], "opened", "timeline: {timeline:?}");
    assert_eq!(timeline[1]["type"], "updated", "timeline: {timeline:?}");
    assert_eq!(timeline[1]["message"], "still investigating");
    assert_eq!(timeline[2]["type"], "closed", "timeline: {timeline:?}");
    assert_eq!(timeline[2]["message"], "resolved: retried webhook secret");
    assert!(incident["closed_at"].as_i64().is_some());
}
