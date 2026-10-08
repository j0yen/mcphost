//! PRD-mcphost-sandbox-return-shape-contract
//! AC5 (P0) -- Given `STARTER_TOOL_SOURCE` and `host.quickstart(kind:
//! python)`, When read, Then the example source contains a
//! `query(...)["rows"]` access with a shape comment, and a test asserts that
//! comment's keys equal the table's.

use crate::common;
use common::{TestServer, extract_structured, python_kind_registry, signup};
use mcphost::kinds::python::{BRIDGE_RETURNS, ReturnShape};
use serde_json::json;
use std::collections::BTreeSet;

fn table_keys(function: &str) -> BTreeSet<String> {
    let row = BRIDGE_RETURNS.iter().find(|r| r.function == function).expect("table row");
    match row.shape {
        ReturnShape::Envelope(keys) => keys.iter().map(|(k, _)| k.to_string()).collect(),
        ReturnShape::Plain(_) => panic!("{function} is not an envelope"),
    }
}

/// Keys of the `{"a": t, "b": t}` object a `returns {...}` comment names.
fn comment_keys(source: &str) -> BTreeSet<String> {
    let line = source
        .lines()
        .find(|l| l.trim_start().starts_with('#') && l.contains("returns {"))
        .unwrap_or_else(|| panic!("no shape comment in the example:\n{source}"));
    let body = &line[line.find("returns {").unwrap() + "returns {".len()..];
    let body = &body[..body.find('}').expect("closing brace")];
    body.split(',').filter_map(|p| p.split(':').next()).map(|k| k.trim().trim_matches('"').to_string()).collect()
}

#[tokio::test]
async fn the_quickstart_python_example_reads_rows_and_states_the_shape_from_the_table() {
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Retshape AC5 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let quickstart = extract_structured(
        &client.tools_call("host.quickstart", json!({"kind": "python"})).await.expect("quickstart kind=python"),
    );
    let source = quickstart["starter_tool"]["spec"]["source"].as_str().expect("starter source");
    // The publish_call carries the same source -- one example, not two.
    assert_eq!(quickstart["starter_tool"]["publish_call"]["arguments"]["spec"]["source"], json!(source));

    let access = source
        .lines()
        .find(|l| !l.trim_start().starts_with('#') && l.contains("query(") && l.contains("[\"rows\"]"))
        .unwrap_or_else(|| panic!("no `query(...)[\"rows\"]` access in the example:\n{source}"));
    assert!(access.contains("mcphost.table.query("), "{access}");

    assert_eq!(comment_keys(source), table_keys("mcphost.table.query"), "shape comment vs table:\n{source}");
}
