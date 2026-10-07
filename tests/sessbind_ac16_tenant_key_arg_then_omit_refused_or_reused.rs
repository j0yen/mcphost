//! PRD-mcphost-session-bound-tenant-key
//! AC3 (P0) — Given a connection that authenticated with tenant A's key on
//! one call, When a later call omits the key, Then the response is
//! `tenant_key_missing` with `data.tenant: "<A>"` and no tenant is created;
//! given `reuse_session_tenant = true`, Then the call runs as A with
//! `auth: "session"`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;
use std::sync::atomic::Ordering;

#[tokio::test]
async fn omitting_the_key_after_an_argument_auth_is_refused_by_default_and_reused_when_configured() {
    let server = TestServer::start().await;

    // Tenant A exists already (signed up out of band, over its own
    // connection) -- this is NOT the connection under test.
    let (namespace, key) = signup(&server.base_url, "AC3 Tenant").await;
    McpClient::with_bearer(&server.base_url, &key)
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "ac16tool",
                "kind": "echo",
                "spec": {"schema": {"type": "object", "properties": {}, "required": []}},
            }),
        )
        .await
        .expect("A publishes a tool over its own key, out of band");

    // The connection under test: one call presents A's key as the
    // tenant_key ARGUMENT (never a header, never signup/host.redeem --
    // resolve_tenant_key_auth's own single write site for
    // AppState::tenant_key_arg_memory).
    let connection = McpClient::new(&server.base_url).with_session_continuity();
    let first = extract_structured(
        &connection
            .tools_call("host.whoami", json!({"tenant_key": key}))
            .await
            .expect("the first call, keyed via the tenant_key argument, must succeed"),
    );
    assert_eq!(first["tenant"].as_str(), Some(namespace.as_str()));

    // When a later call on the SAME connection omits the key: by default,
    // refused and named -- never a tenant created, never A's data touched.
    let second = connection.tools_call("host.usage", json!({})).await;
    let err = second.expect_err("a later key-less call must be refused by default");
    assert_eq!(err.error_code.as_deref(), Some("tenant_key_missing"));
    assert_eq!(
        err.data.get("tenant").and_then(serde_json::Value::as_str),
        Some(namespace.as_str()),
        "data.tenant must name A: {err:?}"
    );
    let tenants = server.state.db.list_tenants().await.expect("list tenants");
    assert_eq!(tenants.len(), 1, "no second tenant must have been created");

    // With reuse_session_tenant = true, the SAME shape of key-less call
    // instead runs as A, with calls.auth_method == "session" (AC3's own
    // "auth: session").
    server.state.reuse_session_tenant.store(true, Ordering::Relaxed);
    let qualified = format!("{namespace}.ac16tool");
    connection
        .tools_call(
            "host.tool_call",
            json!({"name": qualified}),
        )
        .await
        .expect("with reuse_session_tenant = true, the key-less call now runs as A");

    let db_path = server.data_dir.0.join("mcphost.db");
    let conn = rusqlite::Connection::open(&db_path).expect("open raw db");
    let tenant_id: i64 = conn
        .query_row("SELECT id FROM tenants WHERE namespace = ?1", [&namespace], |r| r.get(0))
        .expect("look up tenant id");
    let auth_method: String = conn
        .query_row(
            "SELECT auth_method FROM calls WHERE tenant_id = ?1 ORDER BY id DESC LIMIT 1",
            [tenant_id],
            |r| r.get(0),
        )
        .expect("the reused call must have written a calls row");
    assert_eq!(auth_method, "session", "AC3: the reused call must meter as auth_method session");

    // Still exactly one tenant throughout.
    let tenants = server.state.db.list_tenants().await.expect("list tenants");
    assert_eq!(tenants.len(), 1, "reuse never creates a tenant either");
}
