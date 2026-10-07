//! PRD-mcphost-session-bound-tenant-key
//! AC6 (P1) — Given one blocked second signup today, When `admin.funnel
//! days=1` runs, Then it reports `implicit_second_signup_blocked: 1`; and
//! `host.whoami` on the connection reports `session_tenant`.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn one_blocked_second_signup_is_reported_by_funnel_and_named_by_whoami() {
    let server = TestServer::start().await;
    let session = McpClient::new(&server.base_url).with_session_continuity();

    let first = extract_structured(
        &session
            .tools_call("host.whoami", json!({}))
            .await
            .expect("the first bare call implicitly signs this session up"),
    );
    let namespace = first["tenant"].as_str().expect("tenant").to_string();
    let url = first["onboarding"]["url"]
        .as_str()
        .expect("onboarding.url present on the implicit signup response")
        .to_string();

    // One blocked second signup, today: a second key-less call on the
    // very same connection.
    session
        .tools_call(
            "host.tool_publish",
            json!({"name": "x", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect_err("a second key-less call must be refused");

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let funnel = extract_structured(
        &admin
            .tools_call("admin.funnel", json!({"days": 1}))
            .await
            .expect("admin.funnel"),
    );
    assert_eq!(
        funnel["implicit_second_signup_blocked"].as_i64(),
        Some(1),
        "admin.funnel must report exactly one blocked second signup: {funnel}"
    );

    // host.whoami on the connection, authenticated some OTHER way this
    // time (its own /u/ URL, never the now-refused key-less path) but
    // carrying the SAME server-issued session id, reports session_tenant
    // -- what the connection's own memory remembers, independent of how
    // THIS call itself authenticated.
    let path = url.trim_start_matches(&server.base_url).to_string();
    let url_client = McpClient::new(&server.base_url).with_path(&path).joining_session_of(&session);
    let whoami = extract_structured(
        &url_client
            .tools_call("host.whoami", json!({}))
            .await
            .expect("host.whoami over the connection's own /u/ URL"),
    );
    assert_eq!(whoami["tenant"].as_str(), Some(namespace.as_str()), "sanity: same tenant");
    assert_eq!(
        whoami["session_tenant"].as_str(),
        Some(namespace.as_str()),
        "host.whoami must report session_tenant for this connection's own memory: {whoami}"
    );
}
