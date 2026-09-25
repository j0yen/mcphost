//! PRD-mcphost-alerting-webhook
//! AC6 — Given `MCPHOST_ALERT_TENANT` is set, When any alert is raised,
//! Then that tenant's `host.msg.inbox` returns a message from the system
//! sender with the alert title and id.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn raised_alert_lands_in_the_configured_tenants_inbox() {
    // `AlertConfig::tenant` must be known at server construction, but a
    // real `signup` mints a random namespace -- so the operator tenant is
    // inserted directly (`Db::create_tenant`, same helper `signup`'s own
    // handler calls) under a namespace this test chooses up front, instead
    // of a real `signup` round trip.
    let operator_ns = "t_operator_ac6".to_string();
    let operator_raw_key = "test-operator-ac6-raw-key";

    let alert_config = mcphost::alerts::AlertConfig {
        tenant: Some(operator_ns.clone()),
        ..mcphost::alerts::AlertConfig::default()
    };
    let server = TestServer::start_with_alert_config(alert_config).await;
    server
        .state
        .db
        .create_tenant(
            "Operator Tenant".to_string(),
            operator_ns,
            mcphost::auth::hash_key(operator_raw_key),
            None,
        )
        .await
        .expect("create operator tenant");
    let operator_client = McpClient::with_bearer(&server.base_url, operator_raw_key);

    let alert_id = mcphost::alerts::raise(
        &server.state,
        mcphost::alerts::RaiseInput {
            key: "test.inbox_delivery".to_string(),
            severity: mcphost::alerts::Severity::Warn,
            title: "Test Alert For Inbox".to_string(),
            body: json!({}),
        },
    )
    .await
    .expect("raise");

    let mut found = None;
    for _ in 0..30 {
        let inbox_raw = operator_client
            .tools_call("host.msg.inbox", json!({}))
            .await
            .expect("inbox");
        let inbox = extract_structured(&inbox_raw);
        let messages = inbox["messages"].as_array().cloned().unwrap_or_default();
        if let Some(m) = messages
            .iter()
            .find(|m| m["data"]["alert_id"].as_i64() == Some(alert_id))
        {
            found = Some(m.clone());
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let message = found.expect("the alert notice reached the operator tenant's inbox");
    assert_eq!(message["from_address"], "system");
    assert!(
        message["body"].as_str().unwrap().contains("Test Alert For Inbox"),
        "body must carry the alert title: {message:?}"
    );
    assert!(
        message["body"].as_str().unwrap().contains(&alert_id.to_string()),
        "body must carry the alert id: {message:?}"
    );
    assert_eq!(message["data"]["alert_id"], alert_id);
}
