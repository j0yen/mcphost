//! PRD-mcphost-declared-ordering-per-verb
//! AC5 (P0) -- Given 60 runs of which 7 have `verdict: "exceeded"`, When
//! `host.runs.list(verdict="exceeded", limit=5)` is paged, Then page 1 has 5,
//! page 2 has 2 with no `next_cursor`, and no row is read outside the filter.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup, vord_seed_runs};
use mcphost::db::RunsPageQuery;
use mcphost::paged::RunsOrder;
use serde_json::json;

async fn page(client: &McpClient, cursor: Option<&str>) -> serde_json::Value {
    let mut args = json!({"verdict": "exceeded", "limit": 5});
    if let Some(c) = cursor {
        args["cursor"] = json!(c);
    }
    extract_structured(&client.tools_call("host.runs.list", args).await.expect("runs.list"))
}

#[tokio::test]
async fn verdict_filtered_runs_page_five_then_two_with_no_cursor() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Vord AC5").await;
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("find").expect("tenant");
    let client = McpClient::with_bearer(&server.base_url, &key);
    // The 7 failing runs are the OLDEST: a wider-window-then-filter
    // implementation (fetch limit * 10 = 50 newest, keep matches) finds none.
    let failing = vord_seed_runs(&server, tenant.id, 60, 7).await;

    let p1 = page(&client, None).await;
    let ids1: Vec<&str> = p1["runs"].as_array().expect("runs").iter().map(|r| r["run_id"].as_str().expect("id")).collect();
    assert_eq!(ids1.len(), 5, "page 1: {p1}");
    let cursor = p1["next_cursor"].as_str().expect("page 1 must carry next_cursor");

    let p2 = page(&client, Some(cursor)).await;
    let ids2: Vec<&str> = p2["runs"].as_array().expect("runs").iter().map(|r| r["run_id"].as_str().expect("id")).collect();
    assert_eq!(ids2.len(), 2, "page 2: {p2}");
    assert!(p2.get("next_cursor").is_none(), "last page must omit next_cursor: {p2}");

    let mut walked: Vec<&str> = ids1.iter().chain(ids2.iter()).copied().collect();
    walked.sort_unstable();
    let mut expected: Vec<&str> = failing.iter().map(String::as_str).collect();
    expected.sort_unstable();
    assert_eq!(walked, expected, "the walk is exactly the 7 failing runs, each once");

    // Newest first: created_desc is the default and created_asc reverses it.
    let asc = extract_structured(
        &client
            .tools_call("host.runs.list", json!({"verdict": "exceeded", "limit": 7, "order": "created_asc"}))
            .await
            .expect("asc"),
    );
    let asc_ids: Vec<&str> = asc["runs"].as_array().expect("runs").iter().map(|r| r["run_id"].as_str().expect("id")).collect();
    assert_eq!(asc_ids, failing.iter().map(String::as_str).collect::<Vec<_>>(), "created_asc is insertion order");
    assert_eq!(
        ids1.iter().chain(ids2.iter()).copied().collect::<Vec<_>>(),
        failing.iter().rev().map(String::as_str).collect::<Vec<_>>(),
        "created_desc is newest first"
    );
}

#[tokio::test]
async fn no_row_outside_the_filter_leaves_the_database() {
    let server = TestServer::start().await;
    let (ns, _key) = signup(&server.base_url, "Vord AC5 db").await;
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("find").expect("tenant");
    vord_seed_runs(&server, tenant.id, 60, 7).await;

    // The query layer is asked for limit + 1 = 6 rows and must hand back six
    // rows that all match: a post-filter would return fewer, or rows that do
    // not match.
    let query = RunsPageQuery { verdict: Some("exceeded".to_string()), ..Default::default() };
    let rows = server
        .state
        .db
        .list_runs_page(tenant.id, query, None, RunsOrder::CreatedDesc, 6)
        .await
        .expect("list_runs_page");
    assert_eq!(rows.len(), 6);
    for row in &rows {
        let progress: serde_json::Value = serde_json::from_str(row.run.progress_json.as_deref().expect("progress")).expect("json");
        assert_eq!(progress["budget"]["verdict"], json!("exceeded"), "row outside the filter: {:?}", row.run.id);
    }
}

#[test]
fn the_wider_window_post_filter_is_gone() {
    let src = include_str!("../src/runs.rs");
    assert!(!src.contains("fetch_limit"), "runs.rs must not fetch a wider window");
    assert!(!src.contains("limit * 10"), "runs.rs must not over-fetch and re-cap");
    assert!(!src.contains("run_budget_verdict"), "runs.rs must not filter verdicts in process");
}
