//! PRD-mcphost-host-tool-deprecation AC4 — Given a deprecated field used in
//! a call, When the call succeeds, Then the result envelope contains
//! `deprecations` naming it.

use crate::common;
use common::{TestServer, signup};
use mcphost::api_contract::Deprecation;
use serde_json::json;

fn valid_entry(path: &str) -> Deprecation {
    Deprecation {
        path: path.to_string(),
        since: "2026-01-01".to_string(),
        sunset: "2026-03-02".to_string(),
        replacement: "host.tool_publish.kind_v2".to_string(),
    }
}

#[tokio::test]
async fn a_call_using_a_deprecated_field_succeeds_and_names_it_in_deprecations() {
    let entry = valid_entry("host.tool_publish.kind");
    let server = TestServer::start_with_deprecations(vec![entry.clone()]).await;
    let (_ns, key) = signup(&server.base_url, "AC4 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    // `kind` is required on every host.tool_publish call -- this call
    // necessarily "uses" the deprecated field, and still succeeds exactly
    // as it always has.
    let result = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "ac4_tool", "kind": "echo", "spec": {"schema": {"type": "object", "properties": {}}}}),
        )
        .await
        .expect("host.tool_publish must still succeed for a deprecated-but-present field");
    let structured = common::extract_structured(&result);

    let deprecations = structured["deprecations"]
        .as_array()
        .unwrap_or_else(|| panic!("result envelope must carry deprecations: {structured}"));
    assert_eq!(deprecations.len(), 1, "exactly one field was used: {deprecations:?}");
    assert_eq!(deprecations[0]["path"], "host.tool_publish.kind");
    assert_eq!(deprecations[0]["since"], entry.since);
    assert_eq!(deprecations[0]["sunset"], entry.sunset);
    assert_eq!(deprecations[0]["replacement"], entry.replacement);
}

#[tokio::test]
async fn a_call_not_using_the_deprecated_field_carries_no_deprecations_note() {
    // A different deprecated path (a field this call never passes) must
    // not show up -- `deprecations` only names fields the call actually
    // used.
    let entry = valid_entry("host.tool_publish.some_other_argument");
    let server = TestServer::start_with_deprecations(vec![entry]).await;
    let (_ns, key) = signup(&server.base_url, "AC4 Tenant Clean").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let result = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "ac4_clean_tool", "kind": "echo", "spec": {"schema": {"type": "object", "properties": {}}}}),
        )
        .await
        .expect("host.tool_publish must succeed");
    let structured = common::extract_structured(&result);
    assert!(
        structured.get("deprecations").is_none(),
        "no deprecated field was used: {structured}"
    );
}
