//! AC5 — Given `retry_families()`, When a test compares it with the codes of
//! the error-family table plus `tool_timeout`, Then the sets are equal; and
//! Given a spec with `on: ["429"]`, When published, Then `retry_on_unknown`
//! with `data.accepted` equal to that set.

use crate::common;
use common::{McpClient, TestServer, chain_kind_registry, cretr_chain_spec, signup};
use mcphost::kinds::chain::retry_families;
use mcphost::kinds::http::UPSTREAM_REMEDIES;
use serde_json::json;
use std::collections::BTreeSet;

fn expected_set() -> BTreeSet<String> {
    UPSTREAM_REMEDIES
        .iter()
        .map(|r| r.code.to_string())
        .chain(std::iter::once("tool_timeout".to_string()))
        .collect()
}

#[test]
fn retry_families_equal_the_family_table_plus_tool_timeout() {
    let got: Vec<String> = retry_families().into_iter().map(str::to_string).collect();
    assert_eq!(got.iter().cloned().collect::<BTreeSet<_>>(), expected_set());
    assert_eq!(got.len(), expected_set().len(), "no duplicate family: {got:?}");
    assert!(got.contains(&"upstream_rate_limited".to_string()));
}

#[tokio::test]
async fn publishing_on_429_is_refused_with_the_accepted_set() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Cretr AC5").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = cretr_chain_spec(Some(json!({"max": 1, "backoff_ms": 0, "on": ["429"]})));
    let err = client
        .tools_call("host.tool_publish", json!({"name": "pipe", "kind": "chain", "spec": spec}))
        .await
        .expect_err("`429` is not a family name");
    assert_eq!(err.error_code.as_deref(), Some("retry_on_unknown"), "{err:?}");
    let accepted: BTreeSet<String> = err.data["accepted"]
        .as_array()
        .expect("data.accepted is an array")
        .iter()
        .map(|v| v.as_str().expect("string").to_string())
        .collect();
    assert_eq!(accepted, expected_set());
    assert_eq!(err.data["given"], json!(["429"]));
}
