//! PRD-mcphost-alerting-webhook
//! AC7 — Given three raised alerts, When `admin.alerts.list
//! {unacked_only: true}` is called, Then all three return; after
//! `admin.alerts.ack {id}` on one, the list returns two and `admin_audit`
//! holds the ack entry.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn list_unacked_then_ack_shrinks_it_and_audits() {
    let server = TestServer::start().await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    let mut ids = Vec::new();
    for key in ["test.ac7.one", "test.ac7.two", "test.ac7.three"] {
        let id = mcphost::alerts::raise(
            &server.state,
            mcphost::alerts::RaiseInput {
                key: key.to_string(),
                severity: mcphost::alerts::Severity::Warn,
                title: format!("AC7 alert {key}"),
                body: json!({}),
            },
        )
        .await
        .expect("raise");
        ids.push(id);
    }

    let listed = extract_structured(
        &admin
            .tools_call("admin.alerts.list", json!({"unacked_only": true}))
            .await
            .expect("list"),
    );
    let alerts = listed["alerts"].as_array().expect("alerts array");
    for id in &ids {
        assert!(
            alerts.iter().any(|a| a["id"].as_i64() == Some(*id)),
            "alert {id} must be in the unacked list: {alerts:?}"
        );
    }
    let before_count = alerts.iter().filter(|a| ids.contains(&a["id"].as_i64().unwrap())).count();
    assert_eq!(before_count, 3, "all three raised alerts return unacked");

    let acked_id = ids[0];
    let ack_result = extract_structured(
        &admin
            .tools_call("admin.alerts.ack", json!({"id": acked_id}))
            .await
            .expect("ack"),
    );
    assert_eq!(ack_result["acked"], true);

    let listed_after = extract_structured(
        &admin
            .tools_call("admin.alerts.list", json!({"unacked_only": true}))
            .await
            .expect("list after ack"),
    );
    let alerts_after = listed_after["alerts"].as_array().expect("alerts array");
    let after_count = alerts_after
        .iter()
        .filter(|a| ids.contains(&a["id"].as_i64().unwrap()))
        .count();
    assert_eq!(after_count, 2, "the acked alert must drop out of the unacked list");
    assert!(
        !alerts_after.iter().any(|a| a["id"].as_i64() == Some(acked_id)),
        "the acked alert id must not reappear"
    );

    let audit = extract_structured(
        &admin
            .tools_call("admin.audit_log", json!({}))
            .await
            .expect("audit_log"),
    );
    let entries = audit["entries"].as_array().expect("entries array");
    let acked_id_str = acked_id.to_string();
    assert!(
        entries
            .iter()
            .any(|e| e["action"] == "alerts_ack" && e["target"].as_str() == Some(acked_id_str.as_str())),
        "admin_audit must hold the ack entry: {entries:?}"
    );
}
