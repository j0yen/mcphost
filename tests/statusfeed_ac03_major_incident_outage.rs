//! PRD-mcphost-status-feed AC3 (P0): given
//! `admin.incident.open {title, impact: "major", components: ["mcp"]}`,
//! when `/status.json` is called, then `state` is `"outage"`, the incident
//! appears in `incidents_open`, and `admin_audit` holds the open entry.

use crate::common;

use common::{ADMIN_KEY, McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn major_incident_drives_outage_state() {
    let server = TestServer::start().await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    let opened = admin
        .tools_call(
            "admin.incident.open",
            json!({"title": "MCP endpoint down", "impact": "major", "components": ["mcp"]}),
        )
        .await
        .expect("admin.incident.open");
    let incident_id = extract_structured(&opened)["id"].as_i64().expect("incident id");

    let resp = reqwest::Client::new()
        .get(format!("{}/status.json", server.base_url))
        .send()
        .await
        .expect("GET /status.json");
    let body: serde_json::Value = resp.json().await.expect("parse /status.json");
    assert_eq!(body["state"], "outage", "body: {body}");

    let incidents_open = body["incidents_open"].as_array().expect("incidents_open array");
    assert!(
        incidents_open.iter().any(|i| i["id"] == json!(incident_id)),
        "incidents_open: {incidents_open:?}"
    );

    let audit = admin
        .tools_call("admin.audit_log", json!({}))
        .await
        .expect("admin.audit_log");
    let entries = extract_structured(&audit)["entries"].as_array().cloned().unwrap_or_default();
    let expected_actor = mcphost::auth::hash_key(ADMIN_KEY);
    let open_entry = entries
        .iter()
        .find(|e| e["action"] == json!("incident_open") && e["target"] == json!("MCP endpoint down"))
        .unwrap_or_else(|| panic!("no incident_open audit entry in {entries:?}"));
    assert_eq!(open_entry["actor_key_id"].as_str(), Some(expected_actor.as_str()));
    assert_eq!(open_entry["detail"].as_str(), Some("major"));
}
