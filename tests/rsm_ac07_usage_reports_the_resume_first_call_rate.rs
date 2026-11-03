//! PRD-mcphost-resume-tool AC7 -- Given 10 sessions in 7 days of which 3
//! opened with `host.resume`, When `host.usage` is called, Then
//! `resume.first_call_rate_7d` is 0.3.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::resume::SESSION_GAP_S;
use mcphost::state::now_unix;
use serde_json::json;

fn insert_call(conn: &rusqlite::Connection, tenant_id: i64, tool: &str, at: i64) {
    conn.execute(
        "INSERT INTO calls (tenant_id, tool_name, started_at, started_unix, duration_ms, ok, error_class) \
         VALUES (?1, ?2, 'unix:1.0', ?3, 2, 1, NULL)",
        rusqlite::params![tenant_id, tool, at],
    )
    .unwrap();
}

#[tokio::test]
async fn three_of_ten_sessions_opening_with_resume_is_a_rate_of_point_three() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "RSM AC7").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let conn = rusqlite::Connection::open(server.state.db.data_dir().join("mcphost.db")).unwrap();
    let tenant_id: i64 = conn.query_row("SELECT id FROM tenants WHERE namespace = ?1", [&ns], |r| r.get(0)).unwrap();

    // Ten sessions, one every 12 hours for the last five days, each two calls
    // a minute apart. Three open with host.resume; of the other seven, two
    // call host.resume second -- which does not make them resume-first.
    let start = now_unix() - 5 * 24 * 3600;
    for i in 0..10_i64 {
        let at = start + i * 12 * 3600;
        let (first, second) = match i {
            0 | 4 | 8 => ("host.resume", "echo"),
            2 | 6 => ("host.tool.list", "host.resume"),
            _ => ("host.tool.list", "echo"),
        };
        insert_call(&conn, tenant_id, first, at);
        insert_call(&conn, tenant_id, second, at + 60);
    }
    // A call older than the window is outside the 7 days.
    insert_call(&conn, tenant_id, "host.resume", now_unix() - 8 * 24 * 3600);
    const { assert!(12 * 3600 > SESSION_GAP_S) };

    let out = extract_structured(&client.tools_call("host.usage", json!({})).await.expect("host.usage"));
    assert_eq!(out["resume"]["first_call_rate_7d"], json!(0.3), "{}", out["resume"]);
    assert_eq!(out["resume"]["sessions_7d"], json!(10));
}

#[tokio::test]
async fn no_sessions_means_no_rate() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "RSM AC7 b").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let out = extract_structured(&client.tools_call("host.usage", json!({})).await.expect("host.usage"));
    assert!(out["resume"]["first_call_rate_7d"].is_null(), "{}", out["resume"]);
    assert_eq!(out["resume"]["over_budget_total"], json!(0));
}

#[tokio::test]
async fn a_real_resume_leaves_the_call_the_rate_is_measured_by() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "RSM AC7 c").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client.tools_call("host.resume", json!({})).await.expect("host.resume");
    let out = extract_structured(&client.tools_call("host.usage", json!({})).await.expect("host.usage"));
    assert_eq!(out["resume"]["first_call_rate_7d"], json!(1.0), "{}", out["resume"]);
}
