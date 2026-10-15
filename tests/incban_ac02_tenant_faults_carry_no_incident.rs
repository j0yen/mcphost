//! PRD-mcphost-incident-banner-for-agents
//! AC2 — Given an open incident, When a `tool_exception` or `args_invalid`
//! error is returned, Then its `data` has no `incident` key.

use crate::common;
use common::TestServer;
use mcphost::errors::{AppError, INCIDENT_FIELD};
use serde_json::{Value, json};

fn data_of(err: AppError, base: &str) -> Value {
    err.into_error_data_at(Some(base)).data.expect("error data")
}

#[tokio::test]
async fn incban_ac02() {
    let server = TestServer::start().await;
    let base = server.state.public_url.clone();

    // Opened the way Joe does: `admin.incident.open` refreshes the banner.
    mcphost::admin::incident_open(
        &server.state,
        &json!({"title": "storage stall", "impact": "partial", "components": ["mcp"]}),
    )
    .await
    .expect("open incident");

    // Control: the banner is live, a host-fault error carries it.
    let host = data_of(AppError::Storage("disk".into()), &base);
    assert!(host.get(INCIDENT_FIELD).is_some(), "storage error must carry the open incident: {host}");

    let tool_exception = data_of(
        AppError::Structured {
            code: "tool_exception",
            message: "the tool raised".to_string(),
            data: json!({}),
        },
        &base,
    );
    assert_eq!(tool_exception["error_code"], "tool_exception");
    assert!(tool_exception.get(INCIDENT_FIELD).is_none(), "{tool_exception}");

    let args_invalid = data_of(AppError::InvalidArgs("bad".into()), &base);
    assert_eq!(args_invalid["error_code"], "args_invalid");
    assert!(args_invalid.get(INCIDENT_FIELD).is_none(), "{args_invalid}");

    // Closing the incident drops the banner within one request.
    let id = host[INCIDENT_FIELD]["id"].as_i64().expect("incident id");
    mcphost::admin::incident_close(&server.state, &json!({"id": id, "message": "resolved"}))
        .await
        .expect("close incident");
    let after = data_of(AppError::Storage("disk".into()), &base);
    assert!(after.get(INCIDENT_FIELD).is_none(), "closed incident must clear the banner: {after}");
}
