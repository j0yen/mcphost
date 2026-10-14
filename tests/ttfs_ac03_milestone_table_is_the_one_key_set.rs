//! PRD-mcphost-time-to-first-success-public AC3 (P0) — Given
//! `ACTIVATION_MILESTONES`, When the key sets of `admin.funnel` stages,
//! `whoami.activation` and `onboarding_7d` are collected, Then each equals
//! the table's names and the status block's tool names are a subset of
//! `tools/list`.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup, ttfs_tenant};
use mcphost::funnel::{ACTIVATION_MILESTONES, ADMIN_ONLY_STAGES};
use serde_json::Value;
use std::collections::BTreeSet;

fn table_names() -> BTreeSet<String> {
    ACTIVATION_MILESTONES.iter().map(|(name, _)| name.to_string()).collect()
}

fn keys(v: &Value) -> BTreeSet<String> {
    v.as_object().expect("object").keys().cloned().collect()
}

#[tokio::test]
async fn funnel_whoami_and_onboarding_key_sets_equal_the_table() {
    let server = TestServer::start().await;
    let now = mcphost::state::now_unix();
    let tenant = ttfs_tenant(&server.state.db, "ttfs3-tenant", "external", None, now - 60, [Some(1), None, None, None]).await;

    // admin.funnel: the milestone stages first, then only the admin-only ones.
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let funnel = extract_structured(&admin.tools_call("admin.funnel", serde_json::json!({"days": 7})).await.expect("admin.funnel"));
    let stage_names: Vec<String> =
        funnel["stages"].as_array().expect("stages").iter().map(|s| s["name"].as_str().expect("name").to_string()).collect();
    let expected: Vec<String> =
        ACTIVATION_MILESTONES.iter().map(|(n, _)| n.to_string()).chain(ADMIN_ONLY_STAGES.iter().map(|n| n.to_string())).collect();
    assert_eq!(stage_names, expected, "admin.funnel stages are the table then the admin-only extras");
    let milestone_stages: BTreeSet<String> = stage_names.into_iter().filter(|n| !ADMIN_ONLY_STAGES.contains(&n.as_str())).collect();
    assert_eq!(milestone_stages, table_names());

    // whoami.activation: `<name>_s` per milestone, plus `next_milestone`.
    let whoami = mcphost::control::whoami(&server.state, &tenant, None, "key", None).await.expect("whoami");
    let mut activation_keys = keys(&whoami["activation"]);
    assert!(activation_keys.remove("next_milestone"), "{whoami}");
    let activation_names: BTreeSet<String> =
        activation_keys.iter().map(|k| k.strip_suffix("_s").expect("_s suffix").to_string()).collect();
    assert_eq!(activation_names, table_names());

    // onboarding_7d: `stamped_n` carries one count per milestone.
    let block = mcphost::funnel::onboarding_7d(&server.state.db, now, 1).await.expect("compute");
    assert_eq!(keys(&block["stamped_n"]), table_names());

    // Every tool the status block names is a real tool.
    let (_ns, key) = signup(&server.base_url, "AC3 Tenant").await;
    let listed = McpClient::with_bearer(&server.base_url, &key).tools_list().await.expect("tools/list");
    let listed: BTreeSet<&str> = listed["tools"].as_array().expect("tools").iter().map(|t| t["name"].as_str().expect("name")).collect();
    for (name, tool) in ACTIVATION_MILESTONES.iter().filter(|(_, tool)| !tool.is_empty()) {
        assert!(listed.contains(tool), "milestone {name} names {tool}, absent from tools/list");
    }
}
