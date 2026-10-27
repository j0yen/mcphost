//! PRD-mcphost-org-model
//! AC3 — Given the `ROLES` const, When the drift test runs, Then the
//! migration's CHECK list, every `host.org.*` descriptor's `role` enum, and
//! the const are equal, and `/plans.json` carries `org_members_max` for
//! every plan from the same `Plan` struct.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::org::{ROLES, TOOLS};
use serde_json::Value;

const MIGRATION: &str = include_str!("../migrations/0116_orgs.sql");

/// The quoted names inside the migration's `role ... CHECK (role IN (...))`.
fn migration_check_list() -> Vec<String> {
    let start = MIGRATION.find("CHECK (role IN (").expect("role CHECK in 0116_orgs.sql");
    let rest = &MIGRATION[start + "CHECK (role IN (".len()..];
    let list = &rest[..rest.find(')').expect("closing paren")];
    list.split(',').map(|s| s.trim().trim_matches('\'').to_string()).collect()
}

#[test]
fn orgm_ac03_migration_check_list_equals_roles_const() {
    let expected: Vec<String> = ROLES.iter().map(|r| r.to_string()).collect();
    assert_eq!(migration_check_list(), expected, "0116_orgs.sql CHECK drifted from org::ROLES");
    assert_eq!(
        mcphost::org::roles_sql_list(),
        "'owner', 'member'",
        "the generated SQL list is the const"
    );
}

#[tokio::test]
async fn orgm_ac03_descriptor_role_enums_and_plans_json_match_the_sources() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Drift").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let listed = client.tools_list().await.expect("tools/list");
    let tools = listed["tools"].as_array().expect("tools array");

    let expected: Vec<Value> = ROLES.iter().map(|r| Value::from(*r)).collect();
    let mut enums_seen = 0;
    for name in TOOLS {
        let tool = tools
            .iter()
            .find(|t| t["name"] == *name || t["name"] == name.replace('.', "_"))
            .unwrap_or_else(|| panic!("{name} missing from tools/list"));
        if let Some(role_enum) = tool["inputSchema"]["properties"]["role"].get("enum") {
            enums_seen += 1;
            assert_eq!(role_enum, &Value::Array(expected.clone()), "{name} role enum drifted");
        }
    }
    assert!(enums_seen >= 1, "at least one host.org.* descriptor must carry the role enum");

    let plans: Value = reqwest::get(format!("{}/plans.json", server.base_url))
        .await
        .expect("GET /plans.json")
        .json()
        .await
        .expect("plans.json is JSON");
    let rows = plans["plans"].as_array().expect("plans array");
    assert_eq!(rows.len(), server.state.plans.plans.len());
    for plan in &server.state.plans.plans {
        let row = rows
            .iter()
            .find(|r| r["name"] == plan.name.clone())
            .unwrap_or_else(|| panic!("plan {} missing from /plans.json", plan.name));
        let shown = row
            .pointer("/limits/org_members_max")
            .or_else(|| row.get("org_members_max"))
            .unwrap_or_else(|| panic!("plan {} lacks org_members_max: {row}", plan.name));
        assert_eq!(shown, &Value::from(plan.org_members_max), "plan {}", plan.name);
    }
}
