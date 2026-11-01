//! PRD-mcphost-implicit-signup-mints-only-on-success
//! AC6 (P1) — Given four post-fix implicit tenants (call succeeded; only
//! failed calls; disabled; rolled back), two from one peer, and two R3
//! refusals (fleet, external), When `admin.funnel` runs with `days: 1`, Then
//! `implicit_zero_ok_calls` is 1, `implicit_signup_rolled_back` is 1,
//! `implicit_peer_refused` is `{fleet: 1, external: 1}`,
//! `implicit_tenants_per_peer` max is 2, and `tenant_id` is set on new
//! implicit `signup_events` rows only (rolled-back and pre-fix rows stay NULL).

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured};
use mcphost::implicit_fault::{Fault, arm};
use serde_json::json;

const FLEET_IP: &str = "203.0.113.5";

fn xff(ip: &str) -> [(&str, &str); 1] {
    [("x-forwarded-for", ip)]
}

async fn mint(client: &McpClient, ip: &str) -> String {
    let out = client
        .tools_call_with_headers("host.state.set", json!({"key": "k", "value": 1}), &xff(ip))
        .await
        .expect("a first keyless call mints a tenant");
    extract_structured(&out)["onboarding"]["tenant"].as_str().expect("onboarding.tenant").to_string()
}

#[tokio::test]
async fn funnel_counts_zero_ok_rollbacks_peer_refusals_and_tenants_per_peer() {
    let server = TestServer::start_with_signup_rate_limit_and_fleet_ips(1000, FLEET_IP).await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    // Two implicit tenants from ONE peer (both ran `initialize`): succeeded + disabled.
    let ip1 = "198.51.100.1";
    let s1 = McpClient::new(&server.base_url).with_session_continuity().with_user_agent("imos-ac06/p1");
    let s2 = McpClient::new(&server.base_url).with_session_continuity().with_user_agent("imos-ac06/p1");
    s1.initialize().await;
    s2.initialize().await;
    let _succeeded = mint(&s1, ip1).await;
    let disabled = mint(&s2, ip1).await;
    admin.tools_call("admin.tenant_disable", json!({"tenant": disabled})).await.expect("disable");

    // Only failed calls: an implicit-source tenant whose one call was refused.
    let signed = McpClient::new(&server.base_url).with_user_agent("imos-ac06/p2");
    let signed = extract_structured(&signed.tools_call("signup", json!({"name": "failed-only", "source": "implicit"})).await.expect("signup"));
    let failing = McpClient::with_bearer(&server.base_url, signed["key"].as_str().expect("key"));
    failing.tools_call("host.state.set", json!({"nope": 1})).await.expect_err("a failed call");

    // Rolled back.
    arm("imos-ac06-fault", Fault::Error);
    let faulty = McpClient::new(&server.base_url).with_client_info("imos-ac06-fault", "1").with_user_agent("imos-ac06/p3");
    faulty
        .tools_call_with_headers("host.state.set", json!({"key": "k", "value": 1}), &xff("198.51.100.3"))
        .await
        .expect_err("rolled back");

    // Two R3 refusals: one fleet peer, one external peer.
    for (ua, ip) in [("imos-ac06/fleet", FLEET_IP), ("imos-ac06/ext", "198.51.100.5")] {
        let c = McpClient::new(&server.base_url).with_user_agent(ua);
        mint(&c, ip).await;
        let err = c
            .tools_call_with_headers("host.state.get", json!({"key": "k"}), &xff(ip))
            .await
            .expect_err("second keyless call from the peer is refused");
        assert_eq!(err.error_code.as_deref(), Some("tenant_key_missing"), "{err:?}");
    }

    // An explicit signup (no implicit source) and a pre-fix row stay NULL.
    McpClient::new(&server.base_url).with_user_agent("imos-ac06/explicit").tools_call("signup", json!({"name": "explicit"})).await.expect("signup");
    {
        let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
        conn.execute("INSERT INTO signup_events (source_ip, created_unix) VALUES ('192.0.2.9', strftime('%s','now'))", []).expect("legacy row");
    }

    let funnel = extract_structured(&admin.tools_call("admin.funnel", json!({"days": 1})).await.expect("admin.funnel"));
    assert_eq!(funnel["implicit_zero_ok_calls"], json!(1), "{funnel}");
    assert_eq!(funnel["implicit_signup_rolled_back"], json!(1), "{funnel}");
    assert_eq!(funnel["implicit_peer_refused"], json!({"fleet": 1, "external": 1}), "{funnel}");
    assert_eq!(funnel["implicit_tenants_per_peer"]["max"], json!(2), "{funnel}");

    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let stamped: Vec<(String, String)> = conn
        .prepare("SELECT e.kind, t.signup_source FROM signup_events e JOIN tenants t ON t.id = e.tenant_id")
        .expect("prepare")
        .query_map([], |r| Ok((r.get(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default())))
        .expect("rows")
        .map(|r| r.expect("row"))
        .collect();
    assert_eq!(stamped.len(), 5, "succeeded, disabled, failed-only, fleet and external tenants: {stamped:?}");
    assert!(stamped.iter().all(|(_, source)| source == "implicit"), "only implicit rows carry a tenant: {stamped:?}");
    assert!(stamped.iter().any(|(kind, _)| kind == "implicit_signup_bypassed"), "the fleet signup writes a bypass row");
    let null_rows: i64 = conn.query_row("SELECT COUNT(*) FROM signup_events WHERE tenant_id IS NULL", [], |r| r.get(0)).expect("count");
    assert_eq!(null_rows, 5, "rolled-back, explicit, pre-fix and the two refusal rows stay NULL");
}
