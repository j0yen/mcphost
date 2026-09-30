//! PRD-mcphost-table-concept-graph
//! AC5 — Given `expenses(id, category, amount, day)`, When
//! `host.table.next_questions("expenses")` is called, Then five entries
//! return, each `sql` parses and runs under `host.table.query` on the
//! fixture, and one is a total of `amount` by `category`.
//!
//! `categories(category, group_name)` is this test's own addition to the
//! fixture (not named in the AC's own Given clause), a foreign-key
//! neighbor with its own category-role column -- without it, requirement
//! 5's fourth template ("a measure by a category on a joined table") has
//! no joined table to fire on, and `expenses` alone only ever supports
//! four of the five templates (measure+category, measure+date,
//! measure+id, category-only; see tables_graph.rs's own template list).

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn next_questions_on_expenses_returns_five_runnable_entries() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "TGraph AC5 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.table.create",
            json!({"name": "categories", "columns": {"category": "text", "group_name": "text"}}),
        )
        .await
        .expect("create categories");
    let real_categories = ["food", "travel", "office", "supplies"];
    let groups = ["ops", "fin", "retail", "service"];
    let category_rows: Vec<_> = (0..100)
        .map(|i| {
            let code = if i < real_categories.len() { real_categories[i].to_string() } else { format!("CAT-{i:04}") };
            json!({"category": code, "group_name": groups[i % groups.len()]})
        })
        .collect();
    client
        .tools_call("host.table.append", json!({"table": "categories", "rows": category_rows}))
        .await
        .expect("append categories");

    client
        .tools_call(
            "host.table.create",
            json!({
                "name": "expenses",
                "columns": {"id": "text", "category": "text", "amount": "real", "day": "text"},
            }),
        )
        .await
        .expect("create expenses");
    let expense_rows: Vec<_> = (0..100)
        .map(|i| {
            json!({
                "id": format!("EXP-{i:04}"),
                "category": real_categories[i % real_categories.len()],
                // `% 47` keeps `amount` from being fully unique -- a fully
                // unique numeric column satisfies tables_model.rs's `key`
                // rule before its `measure` rule ever gets checked.
                "amount": 10.0 + (i % 47) as f64,
                "day": format!("2026-02-{:02}", 1 + (i % 25)),
            })
        })
        .collect();
    client
        .tools_call("host.table.append", json!({"table": "expenses", "rows": expense_rows}))
        .await
        .expect("append expenses");

    client.tools_call("host.table.describe", json!({"table": "categories"})).await.expect("describe categories");
    let expenses_model = extract_structured(
        &client.tools_call("host.table.describe", json!({"table": "expenses"})).await.expect("describe expenses"),
    );
    let fks = expenses_model["foreign_keys"].as_array().expect("foreign_keys array");
    assert!(
        fks.iter().any(|fk| fk["column"] == "category" && fk["references_table"] == "categories"),
        "expenses.category -> categories.category must be detected for this fixture: {expenses_model}"
    );

    let result = extract_structured(
        &client.tools_call("host.table.next_questions", json!({"table": "expenses"})).await.expect("next_questions"),
    );
    let questions = result["questions"].as_array().expect("questions array");
    assert_eq!(questions.len(), 5, "expected five next-question entries: {result}");

    let mut found_total_amount_by_category = false;
    for q in questions {
        let sql = q["sql"].as_str().expect("sql string");
        let query_result = client
            .tools_call("host.table.query", json!({"sql": sql}))
            .await
            .unwrap_or_else(|e| panic!("sql must run under host.table.query: {sql}: {} {}", e.code, e.message));
        extract_structured(&query_result)["rows"].as_array().expect("rows array");

        if sql.contains("SUM(\"amount\")") && sql.contains("GROUP BY \"category\"") && !sql.contains("t1") {
            found_total_amount_by_category = true;
        }
    }
    assert!(
        found_total_amount_by_category,
        "expected one entry to be a total of amount by category: {result}"
    );
}
