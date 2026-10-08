//! PRD-mcphost-sandbox-return-shape-contract
//! AC6 (P1) -- Given `host.quickstart(kind: python)`, When called, Then the
//! response carries `bridge_returns` mapping every bridge function to its
//! shape, equal to the table.

use crate::common;
use common::{TestServer, extract_structured, python_kind_registry, signup};
use mcphost::kinds::python::{BRIDGE_ATTRS, BRIDGE_MODULES, BRIDGE_RETURNS, ReturnShape, signature_function};
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[tokio::test]
async fn quickstart_bridge_returns_maps_every_bridge_function_to_the_tables_shape() {
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Retshape AC6 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let quickstart = extract_structured(
        &client.tools_call("host.quickstart", json!({"kind": "python"})).await.expect("quickstart kind=python"),
    );
    let got = quickstart["bridge_returns"].as_object().expect("bridge_returns is an object");

    // Every bridge function the quickstart's own sandbox_api names is mapped.
    let functions: BTreeSet<&str> = BRIDGE_MODULES
        .iter()
        .flat_map(|m| m.signatures.iter().copied())
        .chain(BRIDGE_ATTRS.iter().copied())
        .map(signature_function)
        .collect();
    for f in &functions {
        assert!(got.contains_key(*f), "bridge_returns lacks {f}: {got:?}");
    }

    // ... and each entry equals the table's row.
    assert_eq!(got.len(), BRIDGE_RETURNS.len(), "{got:?}");
    for row in BRIDGE_RETURNS {
        let want: Value = match row.shape {
            ReturnShape::Envelope(keys) => {
                Value::Object(keys.iter().map(|(k, t)| (k.to_string(), json!(t))).collect())
            }
            ReturnShape::Plain(text) => json!(text),
        };
        assert_eq!(got[row.function], want, "{}", row.function);
    }
    assert_eq!(got["mcphost.state.query"], json!({"table": "str", "rows": "list"}));
    assert_eq!(got["mcphost.table.query"], json!({"rows": "list"}));
}
