//! PRD-mcphost-tools-list-alias-truth
//! AC1 — Given the server starts, When tools/list is requested, Then it
//! contains every canonical name, every alias in `tool_aliases.rs`, and the
//! flattened form of each canonical name.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::tool_aliases::{TOOL_ALIASES, flattened_form};
use std::collections::HashSet;

#[tokio::test]
async fn tools_list_has_canonical_alias_and_flattened_names() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AliasTruth AC1").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let listed = client.tools_list().await.expect("tools/list");
    let names: HashSet<&str> =
        listed["tools"].as_array().expect("tools").iter().map(|t| t["name"].as_str().unwrap()).collect();

    let canonicals: Vec<&str> = names
        .iter()
        .copied()
        .filter(|n| (n.starts_with("host.") || n.starts_with("billing.")) && TOOL_ALIASES.iter().all(|a| a.alias != *n))
        .collect();
    assert!(canonicals.len() > 100, "expected the full control plane, got {}", canonicals.len());

    for a in TOOL_ALIASES {
        assert!(names.contains(a.canonical), "canonical {} missing", a.canonical);
        assert!(names.contains(a.alias), "alias {} missing", a.alias);
    }
    for c in &canonicals {
        let flat = flattened_form(c).expect("dotted");
        assert!(names.contains(flat.as_str()), "flattened form {flat} of {c} missing from tools/list");
    }
    assert!(names.contains("host_tool_run") && names.contains("host_bridge_test"));
}
