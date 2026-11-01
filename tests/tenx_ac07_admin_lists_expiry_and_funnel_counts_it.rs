//! PRD-mcphost-synthetic-implicit-tenant-expiry
//! AC7 (P1) — Given one tenant expired by the sweep, one self-offboarded, and
//! 3 backlog rows older than the floor, When `admin.tenants` runs filtered by
//! `disabled_reason: "idle_expired"` then unfiltered, and `admin.funnel` is
//! read, Then the filter returns only the expired one, every unfiltered row
//! carries `expires_at_unix` (null for a non-subject), `tenants_idle_expired`
//! is a `COUNT` of 1 after a restart, and the latest `would_expire` is 3.

use crate::common::tenx::{self, DAY, Fx};
use crate::common::{ADMIN_KEY, EnvGuard, McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

fn namespaces(listing: &Value) -> Vec<String> {
    listing["tenants"]
        .as_array()
        .expect("tenants array")
        .iter()
        .map(|t| t["tenant"].as_str().expect("tenant").to_string())
        .collect()
}

#[tokio::test]
async fn tenx_ac07_admin_filters_by_reason_lists_expiry_and_counts_expired_rows() {
    let _env = EnvGuard::clear(tenx::ENV).await;
    let server = TestServer::start_with_signup_rate_limit(1000).await;
    server.state.db.seed_retention_policy_from_env().await.expect("seed");
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let now = mcphost::state::now_unix();
    tenx::set_floor(&conn, now - 10 * DAY);

    let idle = 2 * DAY;
    tenx::insert(&conn, &Fx::subject("tenx-expired", now, 3, idle));
    tenx::insert(&conn, &Fx::subject("tenx-live-subject", now, 3, 3600));
    for i in 0..3 {
        // Older than the floor: counted by `would_expire`, never touched.
        tenx::insert(&conn, &Fx::subject(&format!("tenx-backlog-{i}"), now, 11, idle));
    }
    let (off_ns, off_key) = signup(&server.base_url, "Tenx Offboarded").await;
    McpClient::with_bearer(&server.base_url, &off_key)
        .tools_call("host.self_offboard", json!({}))
        .await
        .expect("self offboard");

    server.state.db.prune_once().await.expect("prune_once");

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let filtered = extract_structured(
        &admin.tools_call("admin.tenants", json!({"disabled_reason": "idle_expired"})).await.expect("filtered"),
    );
    assert_eq!(namespaces(&filtered), vec!["tenx-expired".to_string()], "only the expired one: {filtered}");

    let all = extract_structured(&admin.tools_call("admin.tenants", json!({})).await.expect("unfiltered"));
    let rows = all["tenants"].as_array().expect("rows");
    assert!(rows.len() >= 6);
    for row in rows {
        assert!(row.as_object().unwrap().contains_key("expires_at_unix"), "every row carries it: {row}");
    }
    let by_ns = |ns: &str| rows.iter().find(|r| r["tenant"] == ns).unwrap_or_else(|| panic!("row {ns}"));
    assert_eq!(by_ns("tenx-live-subject")["expires_at_unix"].as_i64(), Some(now - 3600 + DAY));
    for non_subject in ["tenx-expired", "tenx-backlog-0", off_ns.as_str()] {
        assert!(by_ns(non_subject)["expires_at_unix"].is_null(), "{non_subject} is not a subject");
    }
    assert_eq!(by_ns(&off_ns)["disabled_reason"], "self_offboard");

    let funnel = extract_structured(&admin.tools_call("admin.funnel", json!({})).await.expect("funnel"));
    assert_eq!(funnel["tenants_idle_expired"], 1, "{funnel}");
    // A fresh handle on the same file (a restart) reads the same number: it is a COUNT, not a counter.
    let reopened = mcphost::db::Db::open(&server.data_dir.0).expect("reopen");
    assert_eq!(reopened.count_tenants_idle_expired().await.expect("count"), 1);

    let latest: String = conn
        .query_row("SELECT deleted_json FROM prune_log ORDER BY id DESC LIMIT 1", [], |r| r.get(0))
        .expect("prune_log row");
    let latest: Value = serde_json::from_str(&latest).unwrap();
    assert_eq!(latest["synthetic_implicit_tenants.would_expire"], 3, "{latest}");
    assert_eq!(latest["synthetic_implicit_tenants.expired"], 1);
}
