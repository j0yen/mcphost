//! PRD-mcphost-sandbox-return-shape-contract
//! AC3 (P0) -- Given a sandbox tool that runs `for r in
//! mcphost.state.query(table="t")` or `mcphost.table.query(...)[0]`, When
//! dry-run, Then the verdict is `will_fail` with `BridgeShapeError: …
//! returns {…}; iterate result["rows"]` and the user's line number.

use crate::common;
use common::{TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::{Value, json};

async fn dry_run(source: &str) -> Option<Value> {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return None;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Retshape AC3 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "listy", "kind": "python", "spec": {"source": source}}),
        )
        .await
        .expect("publish ok");
    let result = client
        .tools_call("host.tool_test", json!({"name": "listy", "args": {}}))
        .await
        .unwrap_or_else(|e| panic!("host.tool_test must answer with a verdict: {} {}", e.code, e.message));
    Some(extract_structured(&result))
}

#[tokio::test]
async fn iterating_state_query_is_will_fail_naming_rows_and_the_users_line() {
    let source = r#"import mcphost
def main(args):
    mcphost.state.table_create('t', {'n': 'integer'})
    current = {}
    for r in mcphost.state.query(table='t'):
        current[r['n']] = r
    return current
"#;
    let Some(v) = dry_run(source).await else { return };
    assert_eq!(v["verdict"], json!("will_fail"), "{v}");
    let error = v["error"].as_str().expect("error text");
    assert!(error.starts_with("BridgeShapeError: "), "{error}");
    assert!(error.contains(r#"mcphost.state.query() returns {"table": str, "rows": list}"#), "{error}");
    assert!(error.contains(r#"iterate result["rows"]"#), "{error}");
    assert_eq!(v["line"], json!(5), "the user's `for` line: {v}");
}

#[tokio::test]
async fn indexing_table_query_with_an_integer_is_will_fail_naming_rows_and_the_users_line() {
    let source = r#"import mcphost
def main(args):
    mcphost.table.create('t', {'n': 'integer'})
    mcphost.table.append('t', [{'n': 1}])
    first = mcphost.table.query('SELECT n FROM t')[0]
    return first
"#;
    let Some(v) = dry_run(source).await else { return };
    assert_eq!(v["verdict"], json!("will_fail"), "{v}");
    let error = v["error"].as_str().expect("error text");
    assert!(error.contains(r#"mcphost.table.query() returns {"rows": list}; iterate result["rows"]"#), "{error}");
    assert_eq!(v["line"], json!(5), "{v}");
}
