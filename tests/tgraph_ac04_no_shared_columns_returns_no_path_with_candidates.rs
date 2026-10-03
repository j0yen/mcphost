//! PRD-mcphost-table-concept-graph
//! AC4 — Given two tables with no shared columns, When `join_paths` is
//! called, Then `no_path` returns with each table's key and id columns
//! listed as candidates.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn join_paths_with_no_shared_columns_returns_no_path_and_candidates() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "TGraph AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.table.create",
            json!({"name": "alpha", "columns": {"id": "text", "value": "real"}}),
        )
        .await
        .expect("create alpha");
    let alpha_rows: Vec<_> = (0..50)
        .map(|i| json!({"id": format!("ALPHA-{i:04}"), "value": i as f64}))
        .collect();
    client.tools_call("host.table.append", json!({"table": "alpha", "rows": alpha_rows})).await.expect("append alpha");

    client
        .tools_call(
            "host.table.create",
            json!({"name": "beta", "columns": {"code": "text", "label": "text"}}),
        )
        .await
        .expect("create beta");
    let beta_rows: Vec<_> = (0..50)
        .map(|i| json!({"code": format!("BETA-{i:04}"), "label": format!("Label {i}")}))
        .collect();
    client.tools_call("host.table.append", json!({"table": "beta", "rows": beta_rows})).await.expect("append beta");

    client.tools_call("host.table.describe", json!({"table": "alpha"})).await.expect("describe alpha");
    let beta_model = extract_structured(
        &client.tools_call("host.table.describe", json!({"table": "beta"})).await.expect("describe beta"),
    );
    assert_eq!(
        beta_model["foreign_keys"].as_array().expect("foreign_keys array").len(),
        0,
        "beta must have no detected foreign key in this fixture: {beta_model}"
    );

    let result = extract_structured(
        &client
            .tools_call("host.table.join_paths", json!({"from": "alpha", "to": "beta"}))
            .await
            .expect("host.table.join_paths"),
    );

    assert_eq!(result["no_path"], json!(true), "{result}");
    assert_eq!(result["paths"].as_array().expect("paths array").len(), 0, "{result}");
    let candidates = &result["candidates"];
    let alpha_candidates: Vec<&str> =
        candidates["alpha"].as_array().expect("alpha candidates array").iter().map(|v| v.as_str().unwrap()).collect();
    let beta_candidates: Vec<&str> =
        candidates["beta"].as_array().expect("beta candidates array").iter().map(|v| v.as_str().unwrap()).collect();
    assert!(alpha_candidates.contains(&"id"), "alpha's key column 'id' must be listed as a candidate: {result}");
    assert!(beta_candidates.contains(&"code"), "beta's key column 'code' must be listed as a candidate: {result}");
}
