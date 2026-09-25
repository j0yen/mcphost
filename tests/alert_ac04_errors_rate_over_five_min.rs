//! PRD-mcphost-alerting-webhook
//! AC4 — Given 100 calls in 5 min of which 6 return 5xx-class errors, When
//! the minute tick runs, Then one `errors.rate` alert is raised with the
//! rate in the body; at 4 errors nothing is raised.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

/// Seeds `errors` failed calls tagged `error_class = "upstream_status"` --
/// the real code [`mcphost::kinds::http`] writes for a tool call that got a
/// non-2xx response from its upstream (`src/kinds/http.rs`'s
/// `KindError::structured_with("upstream_status", ...)`), i.e. a genuine
/// 5xx-class tool error, not a fabricated test-only string.
async fn seed_calls(server: &TestServer, tenant_id: i64, total: i64, errors: i64) {
    for i in 0..total {
        let ok = i >= errors;
        server
            .state
            .db
            .record_call(
                tenant_id,
                "echoer".to_string(),
                1,
                ok,
                if ok { None } else { Some("upstream_status".to_string()) },
                None,
                None,
                if ok { "ok" } else { "error" },
                "external".to_string(),
                None,
            )
            .await
            .expect("seed call");
    }
}

/// Seeds `errors` failed calls tagged `error_class = "args_invalid"` -- the
/// real code echo's `Kind::call` writes via `KindError::InvalidArgs` for a
/// client-side malformed-arguments call (`src/kinds/echo.rs:61-66`), a
/// 4xx-class/caller-fault rejection that AC4's "5xx-class tool errors"
/// wording must not count.
async fn seed_client_fault_calls(server: &TestServer, tenant_id: i64, total: i64, errors: i64) {
    for i in 0..total {
        let ok = i >= errors;
        server
            .state
            .db
            .record_call(
                tenant_id,
                "echoer".to_string(),
                1,
                ok,
                if ok { None } else { Some("args_invalid".to_string()) },
                None,
                None,
                if ok { "ok" } else { "error" },
                "external".to_string(),
                None,
            )
            .await
            .expect("seed call");
    }
}

#[tokio::test]
async fn six_of_one_hundred_errors_raises_one_alert() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Error Rate Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "echoer", "kind": "echo", "spec": json!({"schema": {"type": "object"}})}),
        )
        .await
        .expect("publish must succeed");
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .expect("db query")
        .expect("tenant exists");

    seed_calls(&server, tenant.id, 100, 6).await;
    mcphost::alerts::tick_once(&server.state).await.expect("tick");

    let alert = server
        .state
        .db
        .most_recent_alert_for_key("errors.rate".to_string())
        .await
        .expect("query")
        .expect("an errors.rate alert exists at 6%");
    let body: serde_json::Value = serde_json::from_str(&alert.body_json).expect("body json");
    assert_eq!(body["errors"], 6);
    assert_eq!(body["total"], 100);
    assert!(
        body["rate_pct"].as_f64().unwrap() > 5.0,
        "body must carry the actual rate: {body:?}"
    );
}

#[tokio::test]
async fn four_of_one_hundred_errors_raises_nothing() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Below Threshold Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "echoer", "kind": "echo", "spec": json!({"schema": {"type": "object"}})}),
        )
        .await
        .expect("publish must succeed");
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .expect("db query")
        .expect("tenant exists");

    seed_calls(&server, tenant.id, 100, 4).await;
    mcphost::alerts::tick_once(&server.state).await.expect("tick");

    let alert = server
        .state
        .db
        .most_recent_alert_for_key("errors.rate".to_string())
        .await
        .expect("query");
    assert!(alert.is_none(), "4% must not cross the >5% threshold");
}

/// AC4 / requirement 2 pin `errors.rate` to "5xx-class tool errors"
/// specifically. 60 of 100 calls here are `args_invalid` -- a buggy
/// client sending malformed arguments, the caller's own fault -- which
/// must never trip this alert no matter how far over the 5% threshold it
/// pushes the raw `ok = 0` count.
#[tokio::test]
async fn client_fault_errors_alone_raise_nothing() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Buggy Client Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "echoer", "kind": "echo", "spec": json!({"schema": {"type": "object"}})}),
        )
        .await
        .expect("publish must succeed");
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .expect("db query")
        .expect("tenant exists");

    seed_client_fault_calls(&server, tenant.id, 100, 60).await;
    mcphost::alerts::tick_once(&server.state).await.expect("tick");

    let alert = server
        .state
        .db
        .most_recent_alert_for_key("errors.rate".to_string())
        .await
        .expect("query");
    assert!(
        alert.is_none(),
        "args_invalid (4xx-class/caller-fault) errors must never trip errors.rate"
    );
}

/// The 5xx-vs-4xx distinction cuts both ways: mixed in with enough
/// `args_invalid` noise to blow past the threshold on a naive `ok = 0`
/// count, 6 genuine `upstream_status` (5xx-class) errors still raise
/// exactly the alert AC4 promises, counting only the 5xx-class errors in
/// its body.
#[tokio::test]
async fn five_xx_errors_raise_even_alongside_client_fault_noise() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Mixed Errors Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "echoer", "kind": "echo", "spec": json!({"schema": {"type": "object"}})}),
        )
        .await
        .expect("publish must succeed");
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .expect("db query")
        .expect("tenant exists");

    // 6 real 5xx-class errors plus 40 client-fault errors that must be
    // ignored; total call volume stays well above error_rate_min_calls.
    seed_calls(&server, tenant.id, 60, 6).await;
    seed_client_fault_calls(&server, tenant.id, 40, 40).await;
    mcphost::alerts::tick_once(&server.state).await.expect("tick");

    let alert = server
        .state
        .db
        .most_recent_alert_for_key("errors.rate".to_string())
        .await
        .expect("query")
        .expect("6 real 5xx-class errors out of 100 calls must still raise");
    let body: serde_json::Value = serde_json::from_str(&alert.body_json).expect("body json");
    assert_eq!(body["errors"], 6, "only the 5xx-class errors count, not the args_invalid noise");
    assert_eq!(body["total"], 100);
}
