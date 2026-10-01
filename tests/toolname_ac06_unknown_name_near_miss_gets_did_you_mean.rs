//! PRD-mcphost-tool-naming-convention-and-aliases
//! AC6 (P0) -- Given an unknown name one edit away from a canonical
//! (`host.tool.shar`), When called, Then `tool_not_found` includes
//! `did_you_mean: ["host.tool.share"]`.

use crate::common;
use common::{McpClient, TestServer};
use serde_json::json;

#[tokio::test]
async fn one_edit_away_from_a_canonical_name_gets_did_you_mean() {
    let server = TestServer::start().await;
    let (_tenant, key) = common::signup(&server.base_url, "ac06-caller").await;
    let authed = McpClient::with_bearer(&server.base_url, &key);

    let err = authed
        .tools_call("host.tool.shar", json!({}))
        .await
        .expect_err("a near-miss name must still be tool_not_found");
    assert_eq!(err.error_code.as_deref(), Some("tool_not_found"), "{err:?}");
    let hints = err.data["did_you_mean"].as_array().expect("did_you_mean array");
    let hints: Vec<&str> = hints.iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(hints, vec!["host.tool.share"], "{err:?}");
}

#[tokio::test]
async fn an_alias_one_edit_away_also_gets_did_you_mean() {
    let server = TestServer::start().await;
    let (_tenant, key) = common::signup(&server.base_url, "ac06-caller-2").await;
    let authed = McpClient::with_bearer(&server.base_url, &key);

    // "host.tool_shar" is one deletion away from the (still-working) alias
    // host.tool_share -- did_you_mean draws from both sides of the table.
    let err = authed
        .tools_call("host.tool_shar", json!({}))
        .await
        .expect_err("a near-miss of an alias must still be tool_not_found");
    let hints = err.data["did_you_mean"].as_array().expect("did_you_mean array");
    let hints: Vec<&str> = hints.iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(hints, vec!["host.tool_share"], "{err:?}");
}

#[tokio::test]
async fn a_distant_unknown_name_gets_no_did_you_mean() {
    let server = TestServer::start().await;
    let (_tenant, key) = common::signup(&server.base_url, "ac06-caller-3").await;
    let authed = McpClient::with_bearer(&server.base_url, &key);

    let err = authed
        .tools_call("host.totally_unrelated_name", json!({}))
        .await
        .expect_err("an unknown name must be tool_not_found");
    assert_eq!(err.error_code.as_deref(), Some("tool_not_found"), "{err:?}");
    assert!(
        err.data.get("did_you_mean").is_none_or(|v| v.as_array().is_some_and(|a| a.is_empty())),
        "{err:?}"
    );
}
