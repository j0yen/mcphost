//! PRD-mcphost-connection-provider-registry AC6 / R7: an anonymous
//! `host.quickstart` carries `connections.providers_available` and
//! `connections.providers_unavailable`, which together partition
//! `Provider::ALL` (no name in both, none missing, none extra).

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use mcphost::connect::Provider;
use serde_json::json;
use std::collections::BTreeSet;

#[tokio::test]
async fn anonymous_quickstart_lists_available_and_unavailable_partitioning_all() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);
    let result = client
        .tools_call("host.quickstart", json!({}))
        .await
        .expect("anonymous host.quickstart ok");
    let body = extract_structured(&result);
    assert_eq!(body["authenticated"], false, "{body}");

    let connections = &body["connections"];
    let available: Vec<String> = connections["providers_available"]
        .as_array()
        .unwrap_or_else(|| panic!("providers_available present: {connections}"))
        .iter()
        .map(|v| v.as_str().expect("name string").to_string())
        .collect();
    let unavailable_rows = connections["providers_unavailable"]
        .as_array()
        .unwrap_or_else(|| panic!("providers_unavailable present: {connections}"));
    let unavailable: Vec<String> = unavailable_rows
        .iter()
        .map(|r| r["name"].as_str().expect("name string").to_string())
        .collect();
    for row in unavailable_rows {
        assert!(row["reason"].as_str().is_some_and(|r| r.ends_with("_not_configured")), "{row}");
    }

    let a: BTreeSet<&str> = available.iter().map(String::as_str).collect();
    let u: BTreeSet<&str> = unavailable.iter().map(String::as_str).collect();
    assert_eq!(a.len() + u.len(), available.len() + unavailable.len(), "no duplicates");
    assert!(a.is_disjoint(&u), "a provider is in both lists: {a:?} / {u:?}");
    let union: BTreeSet<&str> = a.union(&u).copied().collect();
    let all: BTreeSet<&str> = Provider::ALL.iter().map(|p| p.name()).collect();
    assert_eq!(union, all, "available + unavailable must partition Provider::ALL");
}
