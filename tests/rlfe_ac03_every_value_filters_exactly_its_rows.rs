//! PRD-mcphost-runs-list-filter-enums-generated
//! AC3 (P0) -- Given one stored run per `RunStatus` name, per `RunTrigger`
//! name and per `Verdict` name (written through `as_str()`), When
//! `host.runs.list` is called once per value with that filter, Then each call
//! returns exactly the rows carrying that value (composition rows under
//! `include_children: true`) and the filter-free call returns them all.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::budget::Verdict;
use mcphost::db::TestRunRow;
use mcphost::runs::{RunStatus, RunTrigger};
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn ids(page: &Value) -> BTreeSet<String> {
    page["runs"]
        .as_array()
        .expect("runs array")
        .iter()
        .map(|r| r["run_id"].as_str().expect("run_id").to_string())
        .collect()
}

async fn list(client: &McpClient, mut args: Value) -> BTreeSet<String> {
    args["include_children"] = json!(true);
    args["limit"] = json!(50);
    let page = client.tools_call("host.runs.list", args).await.expect("runs.list ok");
    ids(&extract_structured(&page))
}

#[tokio::test]
async fn rlfe_ac03() {
    let server = TestServer::start().await;
    let db = &server.state.db;
    let mut tenants = Vec::new();
    for label in ["status", "trigger", "verdict"] {
        let (ns, key) = signup(&server.base_url, &format!("RLFE AC3 {label}")).await;
        let tenant = db.find_tenant_by_namespace(ns).await.expect("find").expect("tenant");
        tenants.push((tenant.id, McpClient::with_bearer(&server.base_url, &key)));
    }

    // One run per status (trigger call).
    let (tid, client) = &tenants[0];
    let mut by_status: Vec<(&str, String)> = Vec::new();
    for status in RunStatus::ALL {
        let id = db
            .insert_run_row_for_test(TestRunRow {
                tenant_id: *tid,
                tool_name: "t".into(),
                status: *status,
                trigger: RunTrigger::Call,
                parent_run_id: None,
                progress_json: None,
            })
            .await
            .expect("seed");
        by_status.push((status.as_str(), id));
    }
    for (name, id) in &by_status {
        assert_eq!(list(client, json!({"status": name})).await, BTreeSet::from([id.clone()]), "status {name}");
    }
    let all: BTreeSet<String> = by_status.iter().map(|(_, id)| id.clone()).collect();
    assert_eq!(list(client, json!({})).await, all, "filter-free call returns every status row");

    // One run per trigger (status done); composition rows carry a parent.
    let (tid, client) = &tenants[1];
    let mut by_trigger: Vec<(&str, String)> = Vec::new();
    for trigger in RunTrigger::ALL {
        let parent = (*trigger == RunTrigger::Composition).then(|| "parent-run".to_string());
        let id = db
            .insert_run_row_for_test(TestRunRow {
                tenant_id: *tid,
                tool_name: "t".into(),
                status: RunStatus::Done,
                trigger: *trigger,
                parent_run_id: parent,
                progress_json: None,
            })
            .await
            .expect("seed");
        by_trigger.push((trigger.as_str(), id));
    }
    for (name, id) in &by_trigger {
        assert_eq!(list(client, json!({"trigger": name})).await, BTreeSet::from([id.clone()]), "trigger {name}");
    }
    let all: BTreeSet<String> = by_trigger.iter().map(|(_, id)| id.clone()).collect();
    assert_eq!(list(client, json!({})).await, all, "filter-free call returns every trigger row");

    // One run per verdict, stored in progress.budget.verdict.
    let (tid, client) = &tenants[2];
    let mut by_verdict: Vec<(&str, String)> = Vec::new();
    for name in Verdict::names() {
        let progress = json!({"budget": {"verdict": name}}).to_string();
        let id = db
            .insert_run_row_for_test(TestRunRow {
                tenant_id: *tid,
                tool_name: "t".into(),
                status: RunStatus::Done,
                trigger: RunTrigger::Call,
                parent_run_id: None,
                progress_json: Some(progress),
            })
            .await
            .expect("seed");
        by_verdict.push((name, id));
    }
    for (name, id) in &by_verdict {
        assert_eq!(list(client, json!({"verdict": name})).await, BTreeSet::from([id.clone()]), "verdict {name}");
    }
    let all: BTreeSet<String> = by_verdict.iter().map(|(_, id)| id.clone()).collect();
    assert_eq!(list(client, json!({})).await, all, "filter-free call returns every verdict row");

    // Verdict::NAMES is pinned to as_str(): every variant's string is listed.
    let dim = mcphost::budget::Dimension::ChildCalls;
    for v in [Verdict::Ok, Verdict::Alert, Verdict::Exceeded { dimension: dim }] {
        assert!(Verdict::names().contains(&v.as_str()), "{v:?} missing from Verdict::names()");
    }
    assert_eq!(Verdict::names().len(), 3);
}
