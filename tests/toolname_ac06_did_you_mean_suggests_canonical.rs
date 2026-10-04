//! PRD-mcphost-tool-naming-convention-and-aliases
//! AC6 — Given an unknown name one edit away from a canonical
//! (`host.tool.shar`), When called, Then `tool_not_found` includes
//! `did_you_mean: ["host.tool.share"]`.

use crate::common;
use common::{McpClient, TestServer, signup};

#[tokio::test]
async fn one_edit_away_from_a_canonical_suggests_it() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC6 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call("host.tool.shar", serde_json::json!({}))
        .await
        .expect_err("host.tool.shar must not resolve to any registered tool");

    assert_eq!(err.data["error_code"], "tool_not_found", "{err:?}");
    assert_eq!(
        err.data["did_you_mean"],
        serde_json::json!(["host.tool.share"]),
        "{err:?}"
    );
}

/// The suggestion must never be a deprecated alias (steering a typo
/// toward another old name would defeat this PRD's own point) --
/// `host.tool_shar` (one edit from the alias `host.tool_share`, two edits
/// from the canonical `host.tool.share`) still suggests the canonical.
#[tokio::test]
async fn the_suggestion_is_always_a_canonical_name_never_an_alias() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC6 Tenant Two").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call("host.tool_shar", serde_json::json!({}))
        .await
        .expect_err("host.tool_shar must not resolve to any registered tool");

    assert_eq!(err.data["error_code"], "tool_not_found", "{err:?}");
    let suggestions = err.data["did_you_mean"].as_array().expect("did_you_mean array");
    assert!(
        suggestions.iter().all(|s| mcphost::tool_aliases::resolve(s.as_str().unwrap()).is_none()),
        "no suggestion may be a deprecated alias: {suggestions:?}"
    );
    assert!(
        suggestions.iter().any(|s| s == "host.tool.share"),
        "host.tool.share must be among the suggestions: {suggestions:?}"
    );
}
