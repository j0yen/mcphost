//! PRD-mcphost-refusals-name-their-cause AC4 -- Given the error registry
//! (`src/error_registry.rs`, the `ERROR_CODES` generator behind
//! `/errors.json` and `host.errors`), When `request_body_too_large` is added,
//! Then it is declared through `err_code!` with class, family and remedy so
//! `/errors.json` lists it, the 413 body's `data` carries those fields plus
//! `request_id` like every registered error, and README's request-body line
//! names the body contract.

use crate::common;

use common::{McpClient, TestServer, extract_structured};
use mcphost::error_registry::{ERROR_CODES, ErrorClass, Family, lookup};
use mcphost::http::request_body_too_large_body;
use serde_json::{Value, json};

const CODE: &str = "request_body_too_large";

#[tokio::test]
async fn errors_json_and_host_errors_list_it_and_the_413_body_carries_the_row() {
    let row = lookup(CODE).expect("declared in the registry");
    assert_eq!(row.class, ErrorClass::AgentFault);
    assert_eq!(row.family, Family::Call);
    assert!(!row.remedy.is_empty() && row.remedy.len() <= 200, "{}", row.remedy);
    assert!(ERROR_CODES.iter().any(|e| e.code == CODE));
    // Declared through `err_code!` (its `site` is the declaration's file:line), not a hand row.
    assert!(row.site.starts_with("src/errors.rs:"), "{}", row.site);
    assert_eq!(mcphost::errors::REQUEST_BODY_TOO_LARGE, CODE);

    let server = TestServer::start().await;
    let catalog: Value = reqwest::Client::new()
        .get(format!("{}/errors.json", server.base_url))
        .send()
        .await
        .expect("GET /errors.json")
        .json()
        .await
        .expect("JSON");
    let listed = catalog["codes"]
        .as_array()
        .expect("codes")
        .iter()
        .find(|r| r["code"] == CODE)
        .expect("/errors.json lists the code");
    assert_eq!(listed["class"], "agent_fault");
    assert_eq!(listed["family"], "call");
    assert_eq!(listed["remedy"], row.remedy);
    assert_eq!(listed["retryable"], false);
    assert_eq!(listed["help_url"], format!("{}/help/{CODE}", server.base_url));

    let one = extract_structured(
        &McpClient::new(&server.base_url).tools_call("host.errors", json!({"code": CODE})).await.expect("host.errors"),
    );
    assert_eq!(one["code"], CODE);
    assert_eq!(one["remedy"], row.remedy);

    // The 413 body carries the registry row's fields plus request_id, like every registered error.
    let body = request_body_too_large_body(&server.base_url, Some(1_048_577));
    let data = &body["error"]["data"];
    assert_eq!(data["error_code"], CODE);
    assert_eq!(data["class"], "agent_fault");
    assert_eq!(data["family"], "call");
    assert_eq!(data["remedy"], row.remedy);
    assert_eq!(data["help_url"], format!("{}/help/{CODE}", server.base_url));
    assert!(data["request_id"].as_str().is_some_and(|id| !id.is_empty()), "{data}");
    assert!(data["feedback_hint"].as_str().is_some_and(|h| h.contains(CODE)), "{data}");
    // Two refusals, two correlation ids.
    let again = request_body_too_large_body(&server.base_url, Some(1_048_577));
    assert_ne!(data["request_id"], again["error"]["data"]["request_id"]);
}

#[test]
fn readme_request_body_line_names_the_body_contract() {
    let readme = include_str!("../README.md");
    let start = readme.find("- **Request body**").expect("README has the request-body line");
    let item = &readme[start..];
    let item = &item[..item.find("\n- **").unwrap_or(item.len())];
    for needle in [CODE, "application/json", "limit_bytes", "content_length", "host.blob_put", "request_id", "-32600"] {
        assert!(item.contains(needle), "README's request-body line must name {needle}: {item}");
    }
}
