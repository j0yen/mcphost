//! PRD-mcphost-errors-json-contract
//! AC4 (P0) — Given `host.errors` with no `code` from an anonymous
//! connection and from a keyed tenant, When called, Then both return the same
//! object as `/errors.json`; Given `code: "tool_excepton"`, Then the response
//! is `tool_not_found`-shaped with `did_you_mean: "tool_exception"`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

async fn errors_json(base_url: &str) -> Value {
    reqwest::Client::new()
        .get(format!("{base_url}/errors.json"))
        .send()
        .await
        .expect("GET /errors.json")
        .json()
        .await
        .expect("/errors.json body must be JSON")
}

#[tokio::test]
async fn errjson_ac04_anonymous_and_keyed_host_errors_equal_errors_json() {
    let server = TestServer::start().await;
    let expected = errors_json(&server.base_url).await;
    assert!(expected["codes"].as_array().is_some_and(|c| c.len() > 100), "catalog is non-trivial");

    let anon = McpClient::new(&server.base_url);
    let anon_result = anon.tools_call("host.errors", json!({})).await.expect("anonymous host.errors");
    assert_eq!(extract_structured(&anon_result), expected);

    let (_tenant, key) = signup(&server.base_url, "errjson-ac04").await;
    let keyed = McpClient::with_bearer(&server.base_url, &key);
    let keyed_result = keyed.tools_call("host.errors", json!({})).await.expect("keyed host.errors");
    assert_eq!(extract_structured(&keyed_result), expected);
}

#[tokio::test]
async fn errjson_ac04_misspelt_code_is_tool_not_found_with_did_you_mean() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);
    let err = client
        .tools_call("host.errors", json!({"code": "tool_excepton"}))
        .await
        .expect_err("an unknown code is refused");
    assert_eq!(err.error_code.as_deref(), Some("tool_not_found"), "{err:?}");
    assert_eq!(err.data["did_you_mean"], "tool_exception", "{err:?}");
    assert_eq!(err.data["class"], "agent_fault", "{err:?}");
}

#[tokio::test]
async fn errjson_ac04_a_known_code_returns_its_registry_row() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);
    let result = client
        .tools_call("host.errors", json!({"code": "tool_exception"}))
        .await
        .expect("known code");
    let row = extract_structured(&result);
    let catalog = errors_json(&server.base_url).await;
    let listed = catalog["codes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["code"] == "tool_exception")
        .expect("listed");
    assert_eq!(&row, listed);
}

#[tokio::test]
async fn errjson_ac04_host_errors_is_in_the_anonymous_starter_set() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);
    let list = client.tools_list().await.expect("tools/list");
    let names: Vec<&str> = list["tools"].as_array().unwrap().iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(names.contains(&"host.errors") || names.contains(&"host_errors"), "{names:?}");
}
