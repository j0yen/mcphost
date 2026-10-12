//! PRD-mcphost-declared-ordering-per-verb-b
//! AC1 (P0) -- Given 60 runs of which 7 have `verdict: "fail"`, When
//! `host.runs.list(verdict="fail", limit=5)` is paged, Then page 1 has 5,
//! page 2 has 2 with no `next_cursor`, and no row is read outside the filter.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup, vord_seed_runs};
use mcphost::db::RunsPageQuery;
use mcphost::paged::RunsOrder;
use serde_json::{Value, json};

async fn page(client: &McpClient, cursor: Option<&str>) -> Value {
    let mut args = json!({"verdict": "fail", "limit": 5});
    if let Some(c) = cursor {
        args["cursor"] = json!(c);
    }
    extract_structured(&client.tools_call("host.runs.list", args).await.expect("runs.list"))
}

fn ids(page: &Value) -> Vec<String> {
    page["runs"].as_array().expect("runs").iter().map(|r| r["run_id"].as_str().expect("run_id").to_string()).collect()
}

#[tokio::test]
async fn sixty_runs_seven_failing_page_five_then_two_without_cursor() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Vordb AC1").await;
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("find").expect("tenant");
    let client = McpClient::with_bearer(&server.base_url, &key);
    let failing = vord_seed_runs(&server, tenant.id, 60, 7).await;

    let p1 = page(&client, None).await;
    assert_eq!(ids(&p1).len(), 5, "page 1: {p1}");
    let cursor = p1["next_cursor"].as_str().expect("page 1 carries next_cursor").to_string();
    let p2 = page(&client, Some(&cursor)).await;
    assert_eq!(ids(&p2).len(), 2, "page 2: {p2}");
    assert!(p2.get("next_cursor").is_none(), "last page omits next_cursor: {p2}");

    let mut walked: Vec<String> = ids(&p1).into_iter().chain(ids(&p2)).collect();
    walked.sort();
    let mut expected = failing.clone();
    expected.sort();
    assert_eq!(walked, expected, "exactly the 7 failing runs, once each");
}

#[tokio::test]
async fn the_limit_is_applied_in_sql_over_the_filtered_set() {
    let server = TestServer::start().await;
    let (ns, _key) = signup(&server.base_url, "Vordb AC1 sql").await;
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("find").expect("tenant");
    vord_seed_runs(&server, tenant.id, 60, 7).await;

    let query = RunsPageQuery { verdict: Some("fail".to_string()), ..Default::default() };
    let rows = server.state.db.list_runs_page(tenant.id, query, None, RunsOrder::CreatedDesc, 6).await.expect("list_runs_page");
    assert_eq!(rows.len(), 6, "limit + 1 filtered rows come back from the query itself");
    for row in &rows {
        let progress: Value = serde_json::from_str(row.run.progress_json.as_deref().expect("progress")).expect("json");
        assert_eq!(progress["budget"]["verdict"], json!("fail"), "row outside the filter: {}", row.run.id);
    }
}
