//! PRD-mcphost-table-query-names-missing-table
//! AC7 -- Given `tools/list`, When the `host.table.query` descriptor is read,
//! Then its description (from `host-tools.v1.json` via `gendocs`) names both
//! codes once each, and the generated docs page shows the same sentence.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::Value;

const NAMES: [&str; 2] = ["table_not_found", "table_column_not_found"];

fn assert_names_each_code_once(what: &str, text: &str) {
    for code in NAMES {
        assert_eq!(text.matches(code).count(), 1, "{what} must name {code} exactly once: {text}");
    }
}

#[tokio::test]
async fn tqnf_ac07() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Tqnf AC7 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let listed = client.tools_list().await.expect("tools/list ok");
    let tool = listed["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .find(|t| t["name"] == "host.table.query")
        .expect("host.table.query is listed");
    let description = tool["description"].as_str().expect("description");
    assert_names_each_code_once("tools/list description", description);

    // The committed contract (`mcphost contract dump`) carries the same text...
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let contract: Value = serde_json::from_str(
        &std::fs::read_to_string(root.join("contracts/host-tools.v1.json")).expect("read contract"),
    )
    .expect("contract is JSON");
    let in_contract = contract["tools"]
        .as_array()
        .and_then(|tools| tools.iter().find(|t| t["name"] == "host.table.query"))
        .and_then(|t| t["description"].as_str())
        .expect("contract lists host.table.query with a description");
    assert_eq!(in_contract, description, "contract description drifted from tools/list");

    // ...and `mcphost gen-docs` renders it into the docs page.
    let page = std::fs::read_to_string(root.join("docs/tools.md")).expect("read docs/tools.md");
    let row = page
        .lines()
        .find(|l| l.starts_with("| `host.table.query` |"))
        .expect("docs/tools.md has the host.table.query row");
    assert_names_each_code_once("docs/tools.md row", row);
    let sentence = description.split(". ").last().expect("last sentence");
    assert!(row.contains(sentence.trim_end_matches('.')), "docs page row lacks the sentence {sentence:?}: {row}");
}
