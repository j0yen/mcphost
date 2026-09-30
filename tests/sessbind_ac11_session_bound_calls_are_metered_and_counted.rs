//! PRD-mcphost-session-bound-tenant-after-signup
//! AC11 (P1) — Given the calls in AC 1 and AC 4, When their `calls`/metering
//! rows are read, Then `auth_method` is `session` for AC 1 and `key` for AC
//! 4, and healthz's auth block reports `session_bound_calls_24h` >= 1.
//!
//! Reads `calls.auth_method` back over a raw connection to the server's own
//! database, the same way `oauthsig_ac01_auth_method_by_credential.rs` does
//! for the `{key, issuer_jwt, hosted_token}` values this AC extends. Only
//! `call_published_tool` writes a `calls` row, so both cases drive a real
//! published tool -- `host.whoami`/`host.catalog.search` never write one.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured};
use serde_json::{Value, json};

async fn healthz(base_url: &str) -> Value {
    reqwest::Client::new()
        .get(format!("{base_url}/healthz"))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .expect("GET /healthz")
        .json()
        .await
        .expect("parse /healthz")
}

#[tokio::test]
async fn a_session_bound_call_meters_as_session_and_an_explicit_key_call_as_key() {
    let server = TestServer::start().await;
    let session = McpClient::new(&server.base_url).with_session_continuity();

    let signed_up = extract_structured(
        &session
            .tools_call("signup", json!({"name": "AC11 Tenant"}))
            .await
            .expect("signup"),
    );
    let namespace = signed_up["tenant"].as_str().expect("namespace").to_string();
    let key = signed_up["key"].as_str().expect("key").to_string();

    // Publish key-less on the bound session (AC2's shape), then call the
    // published tool twice: once key-less on the bound session (AC1's shape),
    // once with an explicit `tenant_key` argument (AC4's shape).
    session
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "ac11tool",
                "kind": "echo",
                "spec": {"schema": {"type": "object", "properties": {"msg": {"type": "string"}}}},
            }),
        )
        .await
        .expect("key-less publish on the bound session");

    let qualified = format!("{namespace}.ac11tool");
    session
        .tools_call(&qualified, json!({"msg": "via-session"}))
        .await
        .expect("key-less call on the bound session");

    // AC4's shape: an explicit valid tenant_key. Sent from a connection with
    // no binding at all, so nothing but the argument can resolve it.
    McpClient::new(&server.base_url)
        .tools_call(&qualified, json!({"msg": "via-key", "tenant_key": key}))
        .await
        .expect("explicit-key call");

    let db_path = server.data_dir.0.join("mcphost.db");
    let conn = rusqlite::Connection::open(&db_path).expect("open raw db");
    let tenant_id: i64 = conn
        .query_row("SELECT id FROM tenants WHERE namespace = ?1", [&namespace], |r| r.get(0))
        .expect("look up tenant id");
    let mut stmt = conn
        .prepare("SELECT tool_name, auth_method FROM calls WHERE tenant_id = ?1 ORDER BY id ASC")
        .expect("prepare select");
    let rows: Vec<(String, String)> = stmt
        .query_map([tenant_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .expect("query calls")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect calls rows");

    assert_eq!(
        rows,
        vec![
            ("ac11tool".to_string(), "session".to_string()),
            ("ac11tool".to_string(), "key".to_string()),
        ],
        "AC11: the session-bound call meters as `session`, the explicit-key call as `key`"
    );

    // healthz's auth block counts the session-bound call.
    let health = healthz(&server.base_url).await;
    let oauth = health.get("oauth").unwrap_or_else(|| panic!("missing 'oauth' in {health}"));
    assert!(
        oauth["session_bound_calls_24h"].as_i64().unwrap_or(0) >= 1,
        "healthz's auth block must report session_bound_calls_24h: {oauth}"
    );
}

/// The counter is not a constant: a host that has served no session-bound
/// call reports zero, and the value is a real `calls` read rather than a
/// hardcoded field.
#[tokio::test]
async fn healthz_reports_zero_session_bound_calls_on_a_host_that_has_served_none() {
    let server = TestServer::start().await;
    let (_ns, key) = common::signup(&server.base_url, "AC11 Key-Only Tenant").await;
    let keyed = McpClient::with_bearer(&server.base_url, &key);
    let qualified = common::publish(
        &keyed,
        "keyonly",
        "echo",
        json!({"schema": {"type": "object", "properties": {"msg": {"type": "string"}}}}),
    )
    .await;
    keyed
        .tools_call(&qualified, json!({"msg": "via-key"}))
        .await
        .expect("key-authenticated call");

    let health = healthz(&server.base_url).await;
    let oauth = health.get("oauth").unwrap_or_else(|| panic!("missing 'oauth' in {health}"));
    assert_eq!(
        oauth["session_bound_calls_24h"],
        json!(0),
        "a host whose only calls used a key must report zero session-bound calls: {oauth}"
    );
}
