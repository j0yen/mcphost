//! PRD-mcphost-host-tool-deprecation AC3 — Given the same removal with a
//! valid deprecation entry whose sunset is 60 days out, When `cargo test`
//! runs, Then the test passes and `tools/list` shows `x-deprecated` on that
//! field.
//!
//! "The same removal" here is the announcement half of the lifecycle
//! (requirement 3), not the physical removal AC5 covers: the field stays
//! present and callable, `contracts/deprecations.json` merely carries a
//! valid entry for it ahead of an eventual removal once its `sunset`
//! passes. Two things must hold at that point: the contract test itself
//! must not flag anything (nothing was actually removed), and `tools/list`
//! must surface the announcement.

use crate::common;
use common::{TestServer, signup};
use mcphost::api_contract::{Deprecation, diff, dump_contract};
use mcphost::kinds::KindRegistry;
use serde_json::Value;

fn valid_entry(path: &str) -> Deprecation {
    Deprecation {
        path: path.to_string(),
        since: "2026-01-01".to_string(),
        sunset: "2026-03-02".to_string(), // exactly 60 days after since
        replacement: "host.tool_publish.kind_v2".to_string(),
    }
}

#[test]
fn a_present_field_with_a_valid_entry_produces_no_contract_violation() {
    let contract = dump_contract(&KindRegistry::with_builtin());
    let entry = valid_entry("host.tool_publish.kind");
    assert!(entry.lead_time_valid(), "the fixture entry's own lead time must be valid");

    // Same contract on both sides (the field hasn't actually been
    // removed) -- announcing a deprecation must never itself fail the
    // test.
    let violations = diff(&contract, &contract, &[entry], mcphost::state::now_unix());
    assert!(
        violations.is_empty(),
        "an announced-but-not-removed field must never violate: {violations:?}"
    );
}

#[tokio::test]
async fn tools_list_marks_the_deprecated_field_with_x_deprecated() {
    let entry = valid_entry("host.tool_publish.kind");
    let server = TestServer::start_with_deprecations(vec![entry.clone()]).await;
    let (_ns, key) = signup(&server.base_url, "AC3 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let result = client.tools_list().await.expect("tools/list must succeed");
    let tools = result["tools"].as_array().expect("tools array");
    let tool_publish = tools
        .iter()
        .find(|t| t["name"] == "host.tool_publish")
        .expect("host.tool_publish must be in tools/list");

    let x_deprecated = &tool_publish["inputSchema"]["properties"]["kind"]["x-deprecated"];
    assert_eq!(x_deprecated["since"], Value::from(entry.since.clone()));
    assert_eq!(x_deprecated["sunset"], Value::from(entry.sunset.clone()));
    assert_eq!(
        x_deprecated["replacement"],
        Value::from(entry.replacement.clone())
    );

    let field_description = tool_publish["inputSchema"]["properties"]["kind"]["description"]
        .as_str()
        .expect("kind must still carry a description");
    assert!(
        field_description.contains("DEPRECATED"),
        "the field's own description must note the deprecation too: {field_description}"
    );

    // An unrelated field (and unrelated tools) must be untouched.
    assert!(
        tool_publish["inputSchema"]["properties"]["name"]["x-deprecated"].is_null(),
        "only the named field is annotated"
    );
}
