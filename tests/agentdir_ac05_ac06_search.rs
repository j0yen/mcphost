//! PRD-mcphost-agent-directory
//! AC5 (P0) — Given A has tags `["rag","indexing"]` and description
//! "nightly index builder", When B calls `host.agent.search(tag="rag")`
//! and `host.agent.search(query="INDEX")`, Then both results include A
//! exactly once and exclude a disabled tenant with the same tag.
//! AC6 (P0) — Given 60 profiled tenants, When B calls
//! `host.agent.search(limit=25)` twice following the returned cursor,
//! Then the two pages are disjoint, ordered by handle then namespace, and
//! together contain 50 cards with a third page holding the remaining 10.

use std::collections::HashSet;

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use serde_json::json;

fn count_address(agents: &[serde_json::Value], address: &str) -> usize {
    agents
        .iter()
        .filter(|a| a["address"].as_str() == Some(address))
        .count()
}

#[tokio::test]
async fn ac5_search_by_tag_and_by_query_finds_a_and_excludes_disabled() {
    let server = TestServer::start().await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    let (ns_a, key_a) = signup(&server.base_url, "Indexer Agent").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    client_a
        .tools_call(
            "host.agent.profile_set",
            json!({"tags": ["rag", "indexing"], "description": "nightly index builder"}),
        )
        .await
        .expect("A sets profile");

    let (ns_disabled, key_disabled) = signup(&server.base_url, "Disabled Indexer").await;
    let client_disabled = McpClient::with_bearer(&server.base_url, &key_disabled);
    client_disabled
        .tools_call("host.agent.profile_set", json!({"tags": ["rag"]}))
        .await
        .expect("disabled tenant sets profile before being disabled");
    admin
        .tools_call("admin.tenant_disable", json!({"tenant": ns_disabled.clone()}))
        .await
        .expect("admin.tenant_disable");

    let (_ns_b, key_b) = signup(&server.base_url, "Searcher").await;
    let searcher = McpClient::with_bearer(&server.base_url, &key_b);

    let by_tag_raw = searcher
        .tools_call("host.agent.search", json!({"tag": "rag"}))
        .await
        .expect("search by tag");
    let by_tag = extract_structured(&by_tag_raw);
    let by_tag_agents = by_tag["agents"].as_array().cloned().unwrap_or_default();
    assert_eq!(count_address(&by_tag_agents, &ns_a), 1, "{by_tag_agents:?}");
    assert_eq!(count_address(&by_tag_agents, &ns_disabled), 0, "{by_tag_agents:?}");

    let by_query_raw = searcher
        .tools_call("host.agent.search", json!({"query": "INDEX"}))
        .await
        .expect("search by query");
    let by_query = extract_structured(&by_query_raw);
    let by_query_agents = by_query["agents"].as_array().cloned().unwrap_or_default();
    assert_eq!(count_address(&by_query_agents, &ns_a), 1, "{by_query_agents:?}");
    assert_eq!(count_address(&by_query_agents, &ns_disabled), 0, "{by_query_agents:?}");
}

#[tokio::test]
async fn ac6_search_pages_are_disjoint_ordered_and_exhaustive() {
    // 60 signups from one IP would blow the default 5/hour signup rate
    // limit -- this AC is about search pagination, not signup throughput,
    // same rationale `tests/ac11_load_smoke.rs` documents for seeding many
    // tenants directly.
    let server = TestServer::start_with_signup_rate_limit(100).await;

    // Exactly 60 tenants total, every one profiled -- AC6's own count
    // (25 + 25 + 10) depends on the candidate pool being exactly 60, so the
    // searcher below is one of these 60, not an extra 61st tenant.
    let mut keys = Vec::with_capacity(60);
    for i in 0..60 {
        let (_ns, key) = signup(&server.base_url, &format!("Pageable Agent {i}")).await;
        let client = McpClient::with_bearer(&server.base_url, &key);
        client
            .tools_call("host.agent.profile_set", json!({"handle": format!("page{i:02}")}))
            .await
            .unwrap_or_else(|e| panic!("profile_set for agent {i} failed: {} {}", e.code, e.message));
        keys.push(key);
    }

    let searcher = McpClient::with_bearer(&server.base_url, &keys[0]);

    let page1_raw = searcher
        .tools_call("host.agent.search", json!({"limit": 25}))
        .await
        .expect("page 1");
    let page1 = extract_structured(&page1_raw);
    let page1_agents = page1["agents"].as_array().cloned().unwrap_or_default();
    let cursor1 = page1["cursor"].as_str().expect("page 1 must carry a cursor").to_string();
    assert_eq!(page1_agents.len(), 25, "{page1_agents:?}");

    let page2_raw = searcher
        .tools_call("host.agent.search", json!({"limit": 25, "cursor": cursor1}))
        .await
        .expect("page 2");
    let page2 = extract_structured(&page2_raw);
    let page2_agents = page2["agents"].as_array().cloned().unwrap_or_default();
    let cursor2 = page2["cursor"].as_str().expect("page 2 must carry a cursor").to_string();
    assert_eq!(page2_agents.len(), 25, "{page2_agents:?}");

    let addrs1: HashSet<&str> =
        page1_agents.iter().filter_map(|a| a["address"].as_str()).collect();
    let addrs2: HashSet<&str> =
        page2_agents.iter().filter_map(|a| a["address"].as_str()).collect();
    assert!(addrs1.is_disjoint(&addrs2), "pages must not overlap");

    // Ordering: handle ascending across the whole 50-card span (every one
    // of these 60 tenants claimed a handle, so NULLS-LAST doesn't apply
    // within this test's own data).
    let handles: Vec<&str> = page1_agents
        .iter()
        .chain(page2_agents.iter())
        .filter_map(|a| a["handle"].as_str())
        .collect();
    let mut sorted = handles.clone();
    sorted.sort_unstable();
    assert_eq!(handles, sorted, "results must be ordered by handle");

    let page3_raw = searcher
        .tools_call("host.agent.search", json!({"limit": 25, "cursor": cursor2}))
        .await
        .expect("page 3");
    let page3 = extract_structured(&page3_raw);
    let page3_agents = page3["agents"].as_array().cloned().unwrap_or_default();
    assert_eq!(page3_agents.len(), 10, "{page3_agents:?}");
    assert!(page3["cursor"].is_null(), "the final page must carry no further cursor");
}
