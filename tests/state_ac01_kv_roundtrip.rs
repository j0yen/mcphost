//! PRD-mcphost-tenant-state
//! AC1 — Given a tenant, When `host.state.set(key="a", value={"n": 1})`
//! then `host.state.get(key="a")` run, Then the second returns `{"n": 1}`
//! and `updated_unix` is set. Also covers goal 1's `list`/`delete` (the
//! rest of the key-value namespace `host.state.get/set/delete/list`
//! requirement 2 names together).

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn set_then_get_round_trips_the_value() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Tenant A").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call("host.state.set", json!({"key": "a", "value": {"n": 1}}))
        .await
        .expect("set");

    let got = extract_structured(
        &client
            .tools_call("host.state.get", json!({"key": "a"}))
            .await
            .expect("get"),
    );
    assert_eq!(got["value"], json!({"n": 1}));
    assert_eq!(got["found"], json!(true));
    assert!(
        got["updated_unix"].as_i64().unwrap_or(0) > 0,
        "updated_unix must be set: {got:?}"
    );
}

#[tokio::test]
async fn get_of_an_unset_key_reports_found_false_not_an_error() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Tenant A").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let got = extract_structured(
        &client
            .tools_call("host.state.get", json!({"key": "never-set"}))
            .await
            .expect("get of an unset key must not itself error"),
    );
    assert_eq!(got["found"], json!(false));
    assert_eq!(got["value"], json!(null));
}

#[tokio::test]
async fn delete_then_list_reflects_the_change() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Tenant A").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call("host.state.set", json!({"key": "keep", "value": 1}))
        .await
        .expect("set keep");
    client
        .tools_call("host.state.set", json!({"key": "drop", "value": 2}))
        .await
        .expect("set drop");

    let deleted = extract_structured(
        &client
            .tools_call("host.state.delete", json!({"key": "drop"}))
            .await
            .expect("delete"),
    );
    assert_eq!(deleted["deleted"], json!(true));

    let listed = extract_structured(
        &client
            .tools_call("host.state.list", json!({}))
            .await
            .expect("list"),
    );
    let keys: Vec<String> = listed["keys"]
        .as_array()
        .expect("keys array")
        .iter()
        .map(|k| k["key"].as_str().unwrap().to_string())
        .collect();
    assert!(keys.contains(&"keep".to_string()), "{keys:?}");
    assert!(!keys.contains(&"drop".to_string()), "{keys:?}");

    // Deleting an already-deleted key is not an error; it just reports false.
    let redeleted = extract_structured(
        &client
            .tools_call("host.state.delete", json!({"key": "drop"}))
            .await
            .expect("re-delete"),
    );
    assert_eq!(redeleted["deleted"], json!(false));
}
