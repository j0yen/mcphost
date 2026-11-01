//! PRD-mcphost-external-unclaimed-tenant-expiry
//! AC1 (P0) — Given an external implicit tenant, an external `signup` tenant,
//! a claimed one and a synthetic one, When `host.doctor` and `admin.tenants`
//! are read, the first makes one call, and both are read again, Then both
//! external rows show `tenant_lifecycle.policy = "external_unclaimed"`,
//! `idle_since_unix = MAX(COALESCE(last_seen_unix, created_unix), floor_unix)`
//! and `idle_days`, the call moves the first's clock, and the other two show
//! `policy: null`.

use crate::common::texp::{self, DAY};
use crate::common::{ADMIN_KEY, EnvGuard, McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

async fn doctor(server: &TestServer, key: &str) -> Value {
    let out = McpClient::with_bearer(&server.base_url, key).tools_call("host.doctor", json!({})).await.expect("host.doctor");
    extract_structured(&out)["tenant_lifecycle"].clone()
}

async fn admin_rows(server: &TestServer) -> Vec<Value> {
    let out = McpClient::with_bearer(&server.base_url, ADMIN_KEY).tools_call("admin.tenants", json!({})).await.expect("admin.tenants");
    extract_structured(&out)["tenants"].as_array().expect("tenants").clone()
}

fn row<'a>(rows: &'a [Value], ns: &str) -> &'a Value {
    rows.iter().find(|r| r["tenant"] == ns).unwrap_or_else(|| panic!("{ns} listed"))
}

#[tokio::test]
async fn texp_ac01_external_rows_show_the_policy_clock_and_the_others_show_none() {
    let _env = EnvGuard::clear(texp::ENV).await;
    let server = TestServer::start_with_signup_rate_limit(1000).await;
    server.state.db.seed_retention_policy_from_env().await.expect("seed");
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let now = mcphost::state::now_unix();
    let floor = now - 100 * DAY;
    texp::set_floor(&conn, floor);

    let (implicit_ns, implicit_key) = signup(&server.base_url, "Texp implicit").await;
    let (signup_ns, signup_key) = signup(&server.base_url, "Texp signup").await;
    let (claimed_ns, claimed_key) = signup(&server.base_url, "Texp claimed").await;
    let (synthetic_ns, synthetic_key) = signup(&server.base_url, "Texp synthetic").await;
    let implicit = texp::make_external(&conn, &implicit_ns, now, 5, Some(3));
    // Minted before the floor and never seen: the clock starts at the floor.
    let signed = texp::make_external(&conn, &signup_ns, now, 200, None);
    conn.execute("UPDATE tenants SET signup_source = 'signup' WHERE id = ?1", [signed]).unwrap();
    texp::make_external(&conn, &claimed_ns, now, 5, Some(3));
    conn.execute("UPDATE tenants SET owner_verified_at = ?2, claimed_unix = ?2 WHERE namespace = ?1", rusqlite::params![claimed_ns, now - DAY]).unwrap();
    texp::make_external(&conn, &synthetic_ns, now, 5, Some(3));
    conn.execute("UPDATE tenants SET origin = 'synthetic', synthetic = 'synthorg:test' WHERE namespace = ?1", [&synthetic_ns]).unwrap();

    // Read one: the admin listing, before any tenant call.
    let rows = admin_rows(&server).await;
    let a = row(&rows, &implicit_ns);
    assert_eq!(a["tenant_lifecycle"]["policy"], "external_unclaimed", "{a}");
    assert_eq!(a["lifecycle"], a["tenant_lifecycle"], "lifecycle names the same object");
    assert_eq!(a["idle_since_unix"], now - 3 * DAY);
    assert_eq!(a["idle_days"], 3);
    let b = row(&rows, &signup_ns);
    assert_eq!(b["tenant_lifecycle"]["policy"], "external_unclaimed", "{b}");
    assert_eq!(b["idle_since_unix"], floor, "a pre-floor row's clock starts at the floor");
    assert_eq!(b["idle_days"], 100);
    for ns in [&claimed_ns, &synthetic_ns] {
        let r = row(&rows, ns);
        assert_eq!(r["tenant_lifecycle"]["policy"], Value::Null, "{r}");
        assert_eq!(r["idle_since_unix"], Value::Null, "{r}");
    }

    // The first tenant makes one call: its clock moves to now.
    let before = texp::clock(&conn, implicit).0.expect("last seen");
    let first = doctor(&server, &implicit_key).await;
    let moved = texp::clock(&conn, implicit).0.expect("last seen after");
    assert!(moved > before && moved >= now, "the call moved the clock: {before} -> {moved}");
    assert_eq!(first["policy"], "external_unclaimed");
    assert_eq!(first["idle_since_unix"], moved, "idle_since is the new last_seen");
    assert_eq!(first["idle_days"], 0);
    assert_eq!(first["expires_at_unix"], moved + 30 * DAY);
    assert_eq!(first["warn_at_unix"], moved + 20 * DAY);
    assert_eq!(first["purge_at_unix"], moved + 44 * DAY);

    // The others read their own doctor too.
    let second = doctor(&server, &signup_key).await;
    assert_eq!(second["policy"], "external_unclaimed");
    let second_seen = texp::clock(&conn, signed).0.expect("signup tenant touched");
    assert_eq!(second["idle_since_unix"], second_seen.max(floor));
    for key in [&claimed_key, &synthetic_key] {
        let view = doctor(&server, key).await;
        assert_eq!(view["policy"], Value::Null, "{view}");
    }

    // Read two: admin again; the moved clock shows there.
    let rows = admin_rows(&server).await;
    assert_eq!(row(&rows, &implicit_ns)["idle_since_unix"], moved);
    assert_eq!(row(&rows, &implicit_ns)["tenant_lifecycle"]["policy"], "external_unclaimed");
    assert_eq!(row(&rows, &signup_ns)["idle_since_unix"], second_seen.max(floor));
    for ns in [&claimed_ns, &synthetic_ns] {
        assert_eq!(row(&rows, ns)["tenant_lifecycle"]["policy"], Value::Null);
    }
}
