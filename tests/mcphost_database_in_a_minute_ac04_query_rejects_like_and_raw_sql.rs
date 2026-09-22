//! PRD-mcphost-database-in-a-minute
//! AC4 — Given a `where` with `op` = `LIKE` or a raw SQL string, When
//! `query` is called, Then it returns a validation error and no rows.

use crate::common;
use common::{McpClient, TempDataDir, TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn like_operator_and_raw_sql_where_are_both_refused_with_no_rows() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC4 Owner").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.state.table_create",
            json!({
                "name": "expenses",
                "schema": {"id": "integer", "category": "text", "amount": "real", "day": "integer"},
                "primary_key": "id",
            }),
        )
        .await
        .expect("table create");
    client
        .tools_call(
            "host.state.insert",
            json!({"table": "expenses", "rows": [{"id": 1, "category": "produce", "amount": 11.10, "day": 1}]}),
        )
        .await
        .expect("seed row");

    let query_source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/database-in-a-minute/tools/query.py"),
    )
    .expect("read query.py");
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "query", "kind": "python", "spec": {"source": query_source}}),
        )
        .await
        .expect("publish query");

    let qualified = format!("{ns}.query");

    // Positive control first: a valid where clause returns the seeded row
    // -- proves the tool actually works before proving it refuses.
    let ok = poll_until_ready(
        &client,
        &qualified,
        json!({"where": [{"col": "id", "op": "=", "value": 1}]}),
        Duration::from_secs(10),
    )
    .await
    .unwrap_or_else(|e| panic!("a valid where clause must succeed: {} {}", e.code, e.message));
    let ok_struct = extract_structured(&ok);
    assert_eq!(ok_struct["count"], json!(1), "positive control must return the one seeded row");

    // op = LIKE is refused, no rows.
    let like_err = poll_until_ready(
        &client,
        &qualified,
        json!({"where": [{"col": "category", "op": "LIKE", "value": "%prod%"}]}),
        Duration::from_secs(10),
    )
    .await;
    assert!(
        like_err.is_err(),
        "op = LIKE must be refused with a validation error, got: {like_err:?}"
    );

    // A raw SQL string instead of the structured [{col, op, value}] shape
    // is refused, no rows.
    let raw_sql_err = poll_until_ready(
        &client,
        &qualified,
        json!({"where": "id = 1 OR 1=1"}),
        Duration::from_secs(10),
    )
    .await;
    assert!(
        raw_sql_err.is_err(),
        "a raw SQL where string must be refused with a validation error, got: {raw_sql_err:?}"
    );

    // Positive control again: the table is unaffected -- still exactly
    // the one row a valid query returns, neither rejected call ran
    // anything against it.
    let after = client
        .tools_call(&qualified, json!({}))
        .await
        .expect("unfiltered query must still succeed");
    let after_struct = extract_structured(&after);
    assert_eq!(
        after_struct["count"],
        json!(1),
        "the table must still hold exactly the one seeded row after both refused calls"
    );
}
