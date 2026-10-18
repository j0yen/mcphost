//! PRD mcphost-secret-non-exfiltration-proof AC4 — Given a bound secret and the same mock, When
//! `host.secret.leak_test {name: "x"}` runs, Then it returns `{echo:
//! "blocked", egress: "blocked"}`, the fixture tool is absent from
//! `host.tool.list` afterwards, and `calls` shows exactly two fixture calls.

use crate::common;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
use serde_json::{Value, json};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const SECRET: &str = "sk_live_canary_secret_value";

/// Answers every request with its own body, as JSON.
struct EchoBody;

impl Respond for EchoBody {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        ResponseTemplate::new(200)
            .insert_header("content-type", "application/json")
            .set_body_bytes(request.body.clone())
    }
}

fn fixture_calls(server: &TestServer, tenant_id: i64) -> i64 {
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("open raw db");
    conn.query_row(
        "SELECT COUNT(*) FROM calls WHERE tenant_id = ?1 AND substr(tool_name, 1, 12) = '__leak_test_'",
        [tenant_id],
        |r| r.get(0),
    )
    .expect("count fixture calls")
}

async fn setup(hosts: Option<Value>) -> (TestServer, McpClient, i64) {
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Leak Test Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let mut set = json!({"name": "x", "value": SECRET});
    if let Some(hosts) = hosts {
        set["hosts"] = hosts;
    }
    client.tools_call("host.secret.set", set).await.expect("secret set ok");
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .expect("db ok")
        .expect("tenant exists");
    (server, client, tenant.id)
}

async fn assert_no_fixture_listed(client: &McpClient) {
    let list = client.tools_call("host.tool.list", json!({})).await.expect("tool list ok");
    let dump = extract_structured(&list).to_string();
    assert!(!dump.contains("__leak_test_"), "a fixture tool survived the self-test: {dump}");
}

#[tokio::test]
async fn bound_secret_against_the_echo_mock_blocks_both_legs() {
    let upstream = MockServer::start().await;
    Mock::given(method("POST")).respond_with(EchoBody).mount(&upstream).await;
    let (server, client, tenant_id) = setup(Some(json!(["127.0.0.1"]))).await;

    let result = client
        .tools_call(
            "host.secret.leak_test",
            json!({"name": "x", "echo_url": format!("{}/echo", upstream.uri())}),
        )
        .await
        .expect("leak_test ok");
    let report = extract_structured(&result);
    assert_eq!(report["echo"], "blocked", "{report}");
    assert_eq!(report["egress"], "blocked", "{report}");
    assert_eq!(report["bound_hosts"], json!(["127.0.0.1"]));
    assert!(report["checked_at"].as_i64().unwrap_or(0) > 0);
    assert!(!report.to_string().contains(SECRET));

    // the echo leg really went to the mock; the canary leg sent nothing
    assert_eq!(upstream.received_requests().await.unwrap_or_default().len(), 1);
    assert_no_fixture_listed(&client).await;
    assert_eq!(fixture_calls(&server, tenant_id), 2);
}

#[tokio::test]
async fn default_echo_is_answered_in_process() {
    let (server, client, tenant_id) = setup(Some(json!(["api.example.com"]))).await;
    let result = client
        .tools_call("host.secret.leak_test", json!({"name": "x"}))
        .await
        .expect("leak_test ok");
    let report = extract_structured(&result);
    assert_eq!(report["echo"], "blocked", "{report}");
    assert_eq!(report["egress"], "blocked", "{report}");
    assert_eq!(report["bound_hosts"], json!(["api.example.com"]));
    assert_no_fixture_listed(&client).await;
    assert_eq!(fixture_calls(&server, tenant_id), 2);
}

#[tokio::test]
async fn unbound_secret_skips_the_egress_leg() {
    let (server, client, tenant_id) = setup(None).await;
    let result = client
        .tools_call("host.secret.leak_test", json!({"name": "x"}))
        .await
        .expect("leak_test ok");
    let report = extract_structured(&result);
    assert_eq!(report["echo"], "blocked", "{report}");
    assert_eq!(report["egress"], "unbound", "{report}");
    assert_no_fixture_listed(&client).await;
    assert_eq!(fixture_calls(&server, tenant_id), 1);
}

#[tokio::test]
async fn unknown_secret_is_args_invalid_and_leaves_nothing_behind() {
    let (server, client, tenant_id) = setup(None).await;
    let err = client
        .tools_call("host.secret.leak_test", json!({"name": "nope"}))
        .await
        .expect_err("no such secret");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{err:?}");
    assert_eq!(fixture_calls(&server, tenant_id), 0);
}
