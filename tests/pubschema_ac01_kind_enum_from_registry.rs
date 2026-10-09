//! PRD-mcphost-publish-schema-from-registry
//! AC1 (P0) -- Given tools/list, When `host.tool_publish.inputSchema.
//! properties.kind` is read, Then it has an `enum` equal to
//! `KindRegistry::names()` ∪ job-word aliases ∪ outcome words, derived
//! from the same tables `resolve_kind` and `did_you_mean` read.

use crate::common;
use common::{TempDataDir, TestServer, five_kinds_registry, signup};
use mcphost::kinds::{aliases, outcomes};
use std::collections::BTreeSet;

#[tokio::test]
async fn kind_enum_equals_registry_names_aliases_and_outcome_words() {
    let envs_dir = TempDataDir::new();
    let registry = five_kinds_registry(&envs_dir.0);
    let mut expected: BTreeSet<String> = registry.names().into_iter().map(String::from).collect();
    expected.extend(aliases::alias_names().into_iter().map(String::from));
    for outcome in outcomes::OUTCOMES {
        expected.extend(outcome.words.iter().map(|w| w.to_string()));
    }
    assert!(expected.len() > 10, "expected set looks too small: {expected:?}");

    let server = TestServer::start_with_kinds(registry).await;
    let (_ns, key) = signup(&server.base_url, "Pubschema AC1 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let result = client.tools_list().await.expect("tools/list");
    let publish = result["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .find(|t| t["name"] == "host.tool_publish")
        .expect("host.tool_publish is listed");
    let items = publish["inputSchema"]["properties"]["kind"]["enum"]
        .as_array()
        .expect("kind has an enum");
    let got: BTreeSet<String> = items.iter().map(|v| v.as_str().expect("string").to_string()).collect();
    assert_eq!(got.len(), items.len(), "enum has duplicates: {items:?}");
    assert_eq!(got, expected);
}
