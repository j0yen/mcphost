//! PRD-mcphost-status-feed AC9 (P1): given the alerting registry is present and
//! a component fails 5 consecutive samples, when the tick runs, then one
//! `status.component_down` alert is raised and an incident with
//! `impact: "partial"` is open; after 10 ok samples it is closed with a
//! timeline note.

use crate::common;

use common::{TempDataDir, TestServer, python_kind_registry};

async fn status_component_down_alerts(server: &TestServer) -> Vec<mcphost::db::Alert> {
    server
        .state
        .db
        .list_alerts(100, None, false)
        .await
        .expect("list_alerts")
        .into_iter()
        .filter(|a| a.key == "status.component_down")
        .collect()
}

#[tokio::test]
async fn five_fails_opens_incident_ten_oks_auto_closes() {
    // A real python kind registration (unlike AC2's default TestServer)
    // keeps `exec` genuinely ok throughout, so only the overridden `mcp`
    // component ever crosses the fail/ok streak thresholds -- this AC is
    // about one component's own alert/incident lifecycle, not a second one
    // `exec` would otherwise open for an unrelated reason (no python kind
    // registered).
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    server.state.status_probe_override.set("mcp", false);

    for _ in 0..5 {
        mcphost::statusfeed::tick_once(&server.state)
            .await
            .expect("self-sample tick");
    }

    let alerts = status_component_down_alerts(&server).await;
    assert_eq!(alerts.len(), 1, "alerts: {alerts:?}");

    let body = mcphost::statusfeed::status_json(&server.state)
        .await
        .expect("status_json");
    let open = body["incidents_open"].as_array().expect("incidents_open array");
    let incident = open
        .iter()
        .find(|i| {
            i["components"]
                .as_array()
                .is_some_and(|cs| cs.iter().any(|c| c == "mcp"))
        })
        .unwrap_or_else(|| panic!("no open incident names mcp: {open:?}"));
    assert_eq!(incident["impact"], "partial", "incident: {incident}");
    let incident_id = incident["id"].as_i64().expect("incident id");

    // A tick or two more while still failing must not open a second
    // incident or raise a second alert -- the "no open incident names it"
    // dedupe (requirement 6) holds across ticks.
    mcphost::statusfeed::tick_once(&server.state).await.expect("tick");
    mcphost::statusfeed::tick_once(&server.state).await.expect("tick");
    assert_eq!(status_component_down_alerts(&server).await.len(), 1);
    let body = mcphost::statusfeed::status_json(&server.state).await.expect("status_json");
    let open = body["incidents_open"].as_array().expect("incidents_open array");
    assert_eq!(
        open.iter().filter(|i| i["id"] == serde_json::json!(incident_id)).count(),
        1,
        "open: {open:?}"
    );

    server.state.status_probe_override.set("mcp", true);
    for _ in 0..10 {
        mcphost::statusfeed::tick_once(&server.state)
            .await
            .expect("self-sample tick");
    }

    let body = mcphost::statusfeed::status_json(&server.state)
        .await
        .expect("status_json");
    let open_ids: Vec<i64> = body["incidents_open"]
        .as_array()
        .expect("incidents_open array")
        .iter()
        .map(|i| i["id"].as_i64().expect("id"))
        .collect();
    assert!(!open_ids.contains(&incident_id), "incident should have auto-closed: {open_ids:?}");

    let recent = body["incidents_recent_30d"].as_array().expect("incidents_recent_30d array");
    let closed = recent
        .iter()
        .find(|i| i["id"] == serde_json::json!(incident_id))
        .unwrap_or_else(|| panic!("closed incident not in incidents_recent_30d: {recent:?}"));
    let timeline = closed["timeline"].as_array().expect("timeline array");
    let last = timeline.last().expect("at least one timeline entry");
    assert_eq!(last["type"], "closed", "timeline: {timeline:?}");
    assert!(
        last["message"].as_str().unwrap_or_default().contains("consecutive ok"),
        "close note should mention the consecutive-ok streak: {last}"
    );
}
