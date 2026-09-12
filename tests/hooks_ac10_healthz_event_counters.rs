//! AC10 (P0) — Given the admin `/healthz`, When read after AC1 and AC2,
//! Then `events_received_1h` >= 1 and `events_rejected_1h` >= 1.

use crate::common;
use common::{signup, ADMIN_KEY};
use serde_json::json;

#[tokio::test]
async fn healthz_reports_received_and_rejected_event_counts() {
    let server = common::TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "AC10 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "counted_hook", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    client
        .tools_call("host.secret_set", json!({"name": "tok", "value": "right"}))
        .await
        .expect("secret_set");
    client
        .tools_call(
            "host.trigger.set",
            json!({
                "tool": "counted_hook",
                "kind": "event",
                "verify": {"scheme": "token", "header": "X-Token", "secret": "tok"},
            }),
        )
        .await
        .expect("trigger.set");

    let http = reqwest::Client::new();
    let url = format!("{}/hooks/{}/counted_hook", server.base_url, ns);

    // One accepted event (AC1-shaped).
    let ok = http
        .post(&url)
        .header("X-Token", "right")
        .body("{}")
        .send()
        .await
        .expect("POST /hooks/... (accepted)");
    assert_eq!(ok.status(), 202);

    // One rejected event (AC2-shaped).
    let rejected = http
        .post(&url)
        .header("X-Token", "wrong")
        .body("{}")
        .send()
        .await
        .expect("POST /hooks/... (rejected)");
    assert_eq!(rejected.status(), 401);

    let healthz = http
        .get(format!("{}/healthz", server.base_url))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .expect("GET /healthz");
    assert_eq!(healthz.status(), 200);
    let body: serde_json::Value = healthz.json().await.expect("json body");
    assert!(
        body["events_received_1h"].as_i64().unwrap_or(0) >= 1,
        "events_received_1h: {body:?}"
    );
    assert!(
        body["events_rejected_1h"].as_i64().unwrap_or(0) >= 1,
        "events_rejected_1h: {body:?}"
    );
}
