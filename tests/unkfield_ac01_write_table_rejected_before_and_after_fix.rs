//! AC1 (P0) — Given a python spec with an extra key `write_table`, When
//! `host.tool_publish` or `host.spec_test` runs, Then the error class is
//! `unknown_spec_field`, `data.field == "write_table"`, `data.known` lists
//! the python fields, and no tool is published.
//!
//! This is the PRD's own grounding incident verbatim (2026-09-30 fleet
//! board dogfood: a fictitious `write_table` key published cleanly and the
//! feature it implied never existed). Before this PRD, `PythonSpecRaw`'s
//! `Deserialize` silently drops any key it doesn't declare -- both calls
//! below previously returned `Ok` with `write_table` simply gone.

use crate::common;
use common::{TempDataDir, TestServer, all_kinds_registry, signup};
use serde_json::json;

#[tokio::test]
async fn tool_publish_rejects_an_unknown_python_spec_field() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(all_kinds_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Unkfield AC1a").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "writer",
                "kind": "python",
                "spec": {
                    "source": "def main(args):\n    return {\"ok\": True}\n",
                    "write_table": "runs",
                },
            }),
        )
        .await
        .expect_err("a spec carrying an unknown key must be refused");

    assert_eq!(err.error_code.as_deref(), Some("unknown_spec_field"));
    assert_eq!(err.data["field"], json!("write_table"));
    assert_eq!(err.data["kind"], json!("python"));
    let known = err.data["known"].as_array().expect("known is an array");
    let known: Vec<&str> = known.iter().filter_map(|v| v.as_str()).collect();
    assert!(known.contains(&"source"), "known must list python's real fields: {known:?}");
    assert!(
        !known.contains(&"write_table"),
        "write_table is exactly the field that must NOT be known: {known:?}"
    );

    // Nothing was published.
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .unwrap()
        .expect("tenant exists");
    let tools = server.state.db.list_tools(tenant.id).await.unwrap();
    assert!(tools.is_empty(), "a refused publish must not write a tool row");
}

#[tokio::test]
async fn spec_test_rejects_the_same_unknown_field_before_ever_publishing() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(all_kinds_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Unkfield AC1b").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call(
            "host.spec_test",
            json!({
                "kind": "python",
                "spec": {
                    "source": "def main(args):\n    return {\"ok\": True}\n",
                    "write_table": "runs",
                },
                "invocations": [{}],
            }),
        )
        .await
        .expect_err("host.spec_test must refuse the same unknown key host.tool_publish does");

    assert_eq!(err.error_code.as_deref(), Some("unknown_spec_field"));
    assert_eq!(err.data["field"], json!("write_table"));
}
