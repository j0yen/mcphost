//! PRD-mcphost-org-export-and-offboard-v2
//! AC7 — Given an org export already running, When the owner calls
//! `host.org.export` again, Then the answer is, field for field, what
//! `host.export` answers for its own duplicate on the same tree, and names the
//! running `run_id`.

use crate::common;
use common::TestServer;
use common::orgx::fixture_org;
use common::extract_structured;
use mcphost::export::{EXPORT_TOOL_NAME, ORG_EXPORT_TOOL_NAME};
use serde_json::{Value, json};

#[tokio::test]
async fn orgx_ac07() {
    let server = TestServer::start().await;
    let fx = fixture_org(&server, "acme", 1, 1).await;
    let owner = server.state.db.find_tenant_by_namespace(fx.owner_ns.clone()).await.expect("find").expect("owner");

    // One org export and one tenant export, each already running for the owner.
    let (org_run, tenant_run) = ("01ORGXAC07ORGEXPORTRUN000", "01ORGXAC07TENANTEXPORTRUN0");
    for (run, tool) in [(org_run, ORG_EXPORT_TOOL_NAME), (tenant_run, EXPORT_TOOL_NAME)] {
        server.state.db.start_export_run(owner.id, run.to_string(), tool.to_string(), 300, "{}".to_string()).await.expect("start run");
    }
    let runs_before: i64 = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db"))
        .expect("raw db")
        .query_row("SELECT COUNT(*) FROM runs", [], |r| r.get(0))
        .expect("count");

    let org_dup = extract_structured(&fx.owner.tools_call("host.org.export", json!({})).await.expect("org export duplicate"));
    let own_dup = extract_structured(&fx.owner.tools_call("host.export", json!({})).await.expect("host.export duplicate"));

    assert_eq!(org_dup["run_id"], json!(org_run), "names the running org export: {org_dup}");
    assert_eq!(own_dup["run_id"], json!(tenant_run), "{own_dup}");
    // Field for field the same answer once each names its own running run.
    let with_run = |v: &Value, run: &str| {
        let mut v = v.clone();
        v["run_id"] = json!(if v["run_id"] == json!(run) { "<running>" } else { "<other>" });
        v
    };
    assert_eq!(with_run(&org_dup, org_run), with_run(&own_dup, tenant_run), "same fields and values as host.export's duplicate");
    assert_eq!(org_dup["status"], json!("running"));

    let runs_after: i64 = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db"))
        .expect("raw db")
        .query_row("SELECT COUNT(*) FROM runs", [], |r| r.get(0))
        .expect("count");
    assert_eq!(runs_after, runs_before, "a duplicate starts no second job");
}
