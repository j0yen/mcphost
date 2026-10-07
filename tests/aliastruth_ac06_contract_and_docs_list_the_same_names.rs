//! PRD-mcphost-tools-list-alias-truth
//! AC6 — Given the registry changes, When `sandbox-api-doc-check.sh --write`
//! runs, Then `api_contract` and docs list the same names as tools/list.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::Value;
use std::collections::BTreeSet;
use std::process::Command;

fn read(path: &str) -> String {
    std::fs::read_to_string(format!("{}/{path}", env!("CARGO_MANIFEST_DIR"))).unwrap_or_else(|e| panic!("{path}: {e}"))
}

#[tokio::test]
async fn contract_and_docs_name_exactly_what_tools_list_advertises() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AliasTruth AC6").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let listed = client.tools_list().await.expect("tools/list");
    let advertised: BTreeSet<String> =
        listed["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();

    // The doc-regeneration path the PRD names, run in write mode first: it
    // must leave the committed names untouched (exit 0).
    let status = Command::new("bash")
        .arg(format!("{}/scripts/sandbox-api-doc-check.sh", env!("CARGO_MANIFEST_DIR")))
        .arg("--write")
        .status()
        .expect("run sandbox-api-doc-check.sh --write");
    assert!(status.success(), "sandbox-api-doc-check.sh --write failed: {status}");

    // api_contract: the committed dump.
    let contract: Value = serde_json::from_str(&read("contracts/host-tools.v1.json")).expect("contract json");
    let contract_names: BTreeSet<String> =
        contract["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
    assert_eq!(contract_names, advertised, "api_contract names must equal tools/list");

    // docs/tools.md: canonical column plus aliases column (`signup` is the
    // one anonymous-only tool a tenant's tools/list does not carry).
    let mut doc_names: BTreeSet<String> = BTreeSet::new();
    for row in read("docs/tools.md").lines().filter(|l| l.starts_with("| `")) {
        let cells: Vec<&str> = row.split('|').map(str::trim).collect();
        doc_names.insert(cells[1].trim_matches('`').to_string());
        doc_names.extend(cells[3].split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from));
    }
    doc_names.remove("signup");
    assert_eq!(doc_names, advertised, "docs/tools.md names must equal tools/list");
}
