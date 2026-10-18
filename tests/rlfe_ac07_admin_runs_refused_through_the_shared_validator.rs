//! contract: admin.runs is a checked descriptor that dispatch_admin_tool refuses through host_args::validate_host_args
//! PRD-mcphost-runs-list-filter-enums-generated
//! AC7 (P0) -- Given the admin key, When `admin.runs` is called with
//! `status: "not-a-status"`, Then `dispatch_admin_tool` refuses through the
//! same `host_args::validate_host_args` -- `args_invalid` with
//! `data.path == "/status"`, `data.constraint == "enum"`,
//! `data.expected == RunStatus::names()` -- a counter proves
//! `runs::admin_runs` never ran, and `status: "rejected"` returns the
//! rejected rows.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use mcphost::db::TestRunRow;
use mcphost::host_args::handler_runs;
use mcphost::runs::{RunStatus, RunTrigger};
use serde_json::json;

#[tokio::test]
async fn rlfe_ac07() {
    let server = TestServer::start().await;
    let (ns, _key) = signup(&server.base_url, "RLFE AC7 Tenant").await;
    let tenant = server.state.db.find_tenant_by_namespace(ns.clone()).await.expect("find").expect("tenant");
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    let seed = |status: RunStatus| TestRunRow {
        tenant_id: tenant.id,
        tool_name: "t".into(),
        status,
        trigger: RunTrigger::Event,
        parent_run_id: None,
        progress_json: None,
    };
    let rejected = server.state.db.insert_run_row_for_test(seed(RunStatus::Rejected)).await.expect("seed rejected");
    server.state.db.insert_run_row_for_test(seed(RunStatus::Done)).await.expect("seed done");

    let before = handler_runs(&ns, "admin.runs");
    let err = admin
        .tools_call("admin.runs", json!({"status": "not-a-status", "tenant": ns}))
        .await
        .expect_err("an unknown status must be refused");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{err:?}");
    assert_eq!(err.data["path"], json!("/status"), "{:?}", err.data);
    assert_eq!(err.data["constraint"], json!("enum"), "{:?}", err.data);
    assert_eq!(err.data["expected"], json!(RunStatus::names()), "{:?}", err.data);
    assert_eq!(err.data["tool"], json!("admin.runs"), "{:?}", err.data);
    assert_eq!(handler_runs(&ns, "admin.runs"), before, "admin_runs ran for a refused call");

    let ok = admin.tools_call("admin.runs", json!({"status": "rejected", "tenant": ns})).await.expect("rejected rows");
    let rows = extract_structured(&ok)["runs"].as_array().expect("runs array").clone();
    let ids: Vec<&str> = rows.iter().map(|r| r["run_id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec![rejected.as_str()], "{ok:?}");
    assert_eq!(handler_runs(&ns, "admin.runs"), before + 1, "the counter must see a valid call");
}
