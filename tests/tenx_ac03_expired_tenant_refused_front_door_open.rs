//! PRD-mcphost-synthetic-implicit-tenant-expiry
//! AC3 (P0) — Given an expired tenant, When a call arrives on its
//! `/u/{url_secret}/mcp` URL and another with its key as `tenant_key`, Then
//! both are refused with the error a self-offboarded tenant gets and the
//! reason field reads `idle_expired`, while a keyless call from a different
//! peer still mints a new tenant, so expiry never blocks the front door.

use crate::common::tenx::{self, DAY};
use crate::common::{EnvGuard, McpClient, RpcError, TestServer, extract_structured};
use serde_json::{Value, json};

struct Minted {
    ns: String,
    key: String,
    path: String,
}

/// Implicit-signs-up a tenant keyless, then rotates once to learn its key and
/// current URL (the mint itself only returns `onboarding.url`).
/// Each mint comes from its own User-Agent: the peer-recent guard refuses a second keyless call from a peer that just obtained a tenant.
async fn mint(server: &TestServer, agent: &str) -> Minted {
    let first = extract_structured(
        &McpClient::new(&server.base_url)
            .with_user_agent(agent)
            .tools_call("host.whoami", json!({}))
            .await
            .expect("implicit mint"),
    );
    let ns = first["tenant"].as_str().expect("tenant").to_string();
    let url = first["onboarding"]["url"].as_str().expect("onboarding.url").to_string();
    let by_url = McpClient::new(&server.base_url).with_path(url.trim_start_matches(&server.base_url));
    let rotated = extract_structured(&by_url.tools_call("host.key.rotate", json!({})).await.expect("rotate"));
    Minted {
        ns,
        key: rotated["key"].as_str().expect("key").to_string(),
        path: rotated["url"].as_str().expect("url").trim_start_matches(&server.base_url).to_string(),
    }
}

/// `(error_code, disabled_reason)` the URL path and the `tenant_key` path answer with.
async fn refusals(server: &TestServer, m: &Minted) -> [(Option<String>, Option<String>); 2] {
    let shape = |e: RpcError| (e.error_code.clone(), e.data.get("disabled_reason").and_then(Value::as_str).map(String::from));
    let by_url = McpClient::new(&server.base_url).with_path(&m.path);
    let url_err = by_url.tools_call("host.whoami", json!({})).await.expect_err("URL call is refused");
    let by_key = McpClient::new(&server.base_url);
    let key_err = by_key
        .tools_call("host.whoami", json!({"tenant_key": m.key}))
        .await
        .expect_err("tenant_key call is refused");
    [shape(url_err), shape(key_err)]
}

#[tokio::test]
async fn tenx_ac03_expired_tenant_is_refused_like_an_offboarded_one_and_the_front_door_stays_open() {
    let _env = EnvGuard::clear(tenx::ENV).await;
    let server = TestServer::start_with_signup_rate_limit(1000).await;
    server.state.db.seed_retention_policy_from_env().await.expect("seed");
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let now = mcphost::state::now_unix();
    tenx::set_floor(&conn, now - 10 * DAY);

    let expired = mint(&server, "tenx-ac03-expired").await;
    let offboarded = mint(&server, "tenx-ac03-offboarded").await;
    conn.execute(
        "UPDATE tenants SET signup_source = 'implicit', origin = 'synthetic', synthetic = 'synthorg:test', \
         claimed_unix = NULL, owner_verified_at = NULL, paid_unix = NULL, billing_ref = NULL, \
         created_unix = ?2, last_seen_unix = ?3 WHERE namespace = ?1",
        rusqlite::params![expired.ns, now - 3 * DAY, now - 2 * DAY],
    )
    .expect("make the tenant a subject");
    McpClient::new(&server.base_url)
        .with_path(&offboarded.path)
        .tools_call("host.self.offboard", json!({}))
        .await
        .expect("self offboard");

    server.state.db.prune_once().await.expect("prune_once");

    let gone = refusals(&server, &expired).await;
    let off = refusals(&server, &offboarded).await;
    for path in 0..2 {
        assert_eq!(gone[path].0, off[path].0, "path {path}: same error code as a self-offboarded tenant");
    }
    assert_eq!(gone[0].1.as_deref(), Some("idle_expired"), "URL path names the reason: {gone:?}");
    assert_eq!(off[0].1.as_deref(), Some("self_offboard"), "control: {off:?}");
    if let Some(reason) = &gone[1].1 {
        assert_eq!(reason, "idle_expired");
    }

    // The front door: a keyless call from a peer the expired tenant never saw mints a new tenant.
    let fresh = extract_structured(
        &McpClient::new(&server.base_url)
            .with_user_agent("tenx-ac03-fresh-peer")
            .tools_call("host.whoami", json!({}))
            .await
            .expect("keyless mint"),
    );
    assert_ne!(fresh["tenant"].as_str(), Some(expired.ns.as_str()));
    assert!(fresh["onboarding"]["url"].is_string(), "a new tenant was minted: {fresh}");
}
