//! PRD-mcphost-tenant-delete
//! AC8 — Given a delete, When it completes, Then one structured log line
//! and one `admin_events` row record it, and `admin.usage` totals are
//! unaffected by the `admin_events` row.

mod common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use rusqlite::params;
use serde_json::json;
use std::sync::{Arc, Mutex};
use tracing_subscriber::fmt::MakeWriter;

/// Same capture pattern as `sessionkey_ac17_request_log_no_key.rs`: this
/// must be the only test in this binary that installs a global
/// subscriber.
#[derive(Clone, Default)]
struct BufWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for BufWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("lock buf").extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for BufWriter {
    type Writer = BufWriter;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

fn admin_events_count(db_path: &std::path::Path) -> i64 {
    let conn = rusqlite::Connection::open(db_path).expect("open raw db");
    conn.query_row("SELECT COUNT(*) FROM admin_events", params![], |r| {
        r.get(0)
    })
    .expect("count admin_events")
}

#[tokio::test]
async fn delete_writes_one_log_line_one_admin_event_and_leaves_usage_untouched() {
    let buf = BufWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_writer(buf.clone())
        .with_target(false)
        .finish();
    tracing::subscriber::set_global_default(subscriber)
        .expect("install the test's global tracing subscriber");

    let server = TestServer::start().await;
    let db_path = server.data_dir.0.join("mcphost.db");
    let (tenant_ns, _key) = signup(&server.base_url, "Audited Tenant").await;
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(tenant_ns.clone())
        .await
        .unwrap()
        .expect("tenant");
    server
        .state
        .db
        .record_call(tenant.id, "tool0".to_string(), 5, true, None, None, None, "ok", "external".to_string(), None)
        .await
        .expect("record_call");

    let before_events = admin_events_count(&db_path);
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    // Baseline: another (surviving) tenant's usage total, read before the
    // delete, so the assertion below proves the *admin_events row itself*
    // doesn't leak into any usage aggregate -- not just that the deleted
    // tenant's own calls are gone (AC1 already covers that).
    let (other_ns, _) = signup(&server.base_url, "Bystander Tenant").await;
    let other_tenant = server
        .state
        .db
        .find_tenant_by_namespace(other_ns)
        .await
        .unwrap()
        .expect("other tenant");
    server
        .state
        .db
        .record_call(other_tenant.id, "toolX".to_string(), 5, true, None, None, None, "ok", "external".to_string(), None)
        .await
        .expect("record_call for bystander");
    let usage_before = admin
        .tools_call("admin.usage", json!({}))
        .await
        .expect("admin.usage before");
    let usage_before = extract_structured(&usage_before);
    let calls_before: i64 = usage_before["usage"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["calls"].as_i64().unwrap_or(0))
        .sum();

    admin
        .tools_call("admin.tenant_delete", json!({"tenant": tenant_ns}))
        .await
        .expect("admin.tenant_delete");

    let after_events = admin_events_count(&db_path);
    assert_eq!(
        after_events,
        before_events + 1,
        "exactly one admin_events row must be written per delete"
    );

    let captured = String::from_utf8(buf.0.lock().expect("lock buf").clone())
        .expect("captured log is UTF-8");
    let delete_lines: Vec<&str> = captured
        .lines()
        .filter(|line| line.contains("\"action\":\"tenant_delete\""))
        .collect();
    assert_eq!(
        delete_lines.len(),
        1,
        "exactly one structured log line must record the delete: {captured}"
    );

    // admin.usage's total is unaffected by the admin_events row: the
    // bystander tenant's one call is still the only thing counted (the
    // deleted tenant's own call is gone via cascade, per AC1).
    let usage_after = admin
        .tools_call("admin.usage", json!({}))
        .await
        .expect("admin.usage after");
    let usage_after = extract_structured(&usage_after);
    let calls_after: i64 = usage_after["usage"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["calls"].as_i64().unwrap_or(0))
        .sum();
    assert_eq!(
        calls_after,
        calls_before - 1,
        "admin.usage must drop by exactly the deleted tenant's own call, not be perturbed by the admin_events row"
    );
}
