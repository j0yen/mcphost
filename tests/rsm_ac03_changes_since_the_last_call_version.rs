//! PRD-mcphost-resume-tool AC3 -- Given a tenant whose last session ran on
//! host version A and the host now runs B > A, When `host.resume` is called,
//! Then `host.version_at_last_call` is A, `host.changes` equals
//! `host.changelog {since: A}` cut to 5, and afterwards
//! `tenants.last_seen_version` is B.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::resume::BUDGET;
use serde_json::{Value, json};

const A: &str = "0.100.0";
const B: &str = env!("CARGO_PKG_VERSION");

fn last_seen_version(server: &TestServer, ns: &str) -> Option<String> {
    let conn = rusqlite::Connection::open(server.state.db.data_dir().join("mcphost.db")).unwrap();
    conn.query_row("SELECT last_seen_version FROM tenants WHERE namespace = ?1", [ns], |r| r.get(0)).unwrap()
}

#[tokio::test]
async fn changes_are_the_changelog_since_a_cut_to_five_and_the_column_moves_to_b() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "RSM AC3").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let conn = rusqlite::Connection::open(server.state.db.data_dir().join("mcphost.db")).unwrap();
    conn.execute("UPDATE tenants SET last_seen_version = ?1 WHERE namespace = ?2", [A, ns.as_str()]).unwrap();

    let out = extract_structured(&client.tools_call("host.resume", json!({})).await.expect("host.resume"));
    assert_eq!(out["host"]["version"], B);
    assert_eq!(out["host"]["version_at_last_call"], A);

    // The column is stamped by the call itself, after the previous value was read.
    assert_eq!(last_seen_version(&server, &ns).as_deref(), Some(B));

    let log = extract_structured(&client.tools_call("host.changelog", json!({"since": A})).await.expect("changelog"));
    let all = log["releases"].as_array().expect("releases");
    assert!(all.len() > BUDGET.changes, "the fixture needs more than {} releases since {A}", BUDGET.changes);
    let expected: Vec<Value> = all.iter().take(BUDGET.changes).cloned().collect();
    assert_eq!(out["host"]["changes"], json!(expected));
    assert_eq!(out["truncated"]["changes"], true);
}

#[tokio::test]
async fn a_tenant_already_on_b_sees_no_changes() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "RSM AC3 b").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    // Any call stamps B; the resume after it has nothing newer to report.
    client.tools_call("host.whoami", json!({})).await.expect("whoami");
    let out = extract_structured(&client.tools_call("host.resume", json!({})).await.expect("host.resume"));
    assert_eq!(out["host"]["version_at_last_call"], B);
    assert_eq!(out["host"]["changes"], json!([]));
    assert_eq!(out["truncated"]["changes"], false);
}
