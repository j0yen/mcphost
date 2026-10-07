//! PRD-mcphost-tools-list-alias-truth
//! AC7 — Given an alias entry in tools/list, When read, Then it carries
//! `alias_of: <canonical>`.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::tool_aliases::{TOOL_ALIASES, flattened_form};
use serde_json::Value;
use std::collections::HashMap;

#[tokio::test]
async fn dotted_and_flattened_alias_entries_point_at_their_canonical() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AliasTruth AC7").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let listed = client.tools_list().await.expect("tools/list");
    let by_name: HashMap<&str, &Value> =
        listed["tools"].as_array().unwrap().iter().map(|t| (t["name"].as_str().unwrap(), t)).collect();

    for a in TOOL_ALIASES {
        assert_eq!(by_name[a.alias]["_meta"]["alias_of"], a.canonical, "{}", a.alias);
        assert!(by_name[a.canonical]["_meta"]["alias_of"].is_null(), "canonical {} must not carry alias_of", a.canonical);
    }
    let flat = flattened_form("host.tool.run").unwrap();
    assert_eq!(by_name[flat.as_str()]["_meta"]["alias_of"], "host.tool.run");
    assert_eq!(by_name["host_bridge_test"]["_meta"]["alias_of"], "host.bridge.test");
}
