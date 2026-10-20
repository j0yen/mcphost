//! PRD-mcphost-email-send-live-proof
//! AC4 (P0) -- Given `VERIFICATION_METHODS` carrying `("admin",
//! "admin.tenant_verify_owner")`, When the drift test runs, Then the method
//! enum, the migration CHECK and the `tenant_audit` kind list each include
//! `admin` from the table, `/claim/methods.json` and `host.quickstart.next`
//! list exactly the `host.`-prefixed entries, a table entry whose tool has no
//! dispatch arm fails the test, and `tools/list` on `/mcp` for a tenant key
//! and for no key contains neither `admin.tenant_verify_owner` nor any alias
//! of it while the admin key's list does.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use mcphost::verify::{VERIFICATION_METHODS, agent_startable, audit_kinds, method_names};
use serde_json::{Value, json};

const ADMIN_TOOL: &str = "admin.tenant_verify_owner";

fn tool_names(list: &Value) -> Vec<String> {
    list["tools"].as_array().expect("tools array").iter().map(|t| t["name"].as_str().unwrap_or_default().to_string()).collect()
}

#[test]
fn emslp_ac04_the_table_carries_admin_and_the_migration_check_equals_it() {
    assert!(VERIFICATION_METHODS.contains(&("admin", ADMIN_TOOL)));
    // The newest migration to touch the CHECK is the one that must equal the table.
    let sql = include_str!("../migrations/0125_owner_method_admin.sql");
    let check = sql.lines().find(|l| l.contains("owner_method TEXT CHECK")).expect("CHECK line");
    let listed = check.split_once("IN (").expect("IN list").1.split(')').next().unwrap();
    let in_migration: Vec<&str> = listed.split(',').map(|s| s.trim().trim_matches('\'')).collect();
    assert_eq!(in_migration, method_names());
    assert!(in_migration.contains(&"admin"));
    assert_eq!(audit_kinds(), method_names().iter().map(|m| format!("claim.{m}")).collect::<Vec<_>>());
    assert!(audit_kinds().contains(&"claim.admin".to_string()));
}

#[tokio::test]
async fn emslp_ac04_enum_methods_json_and_quickstart_list_the_generated_sets() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "emslp ac4 lists").await;
    let tenant = McpClient::with_bearer(&server.base_url, &key);
    let want: Vec<Value> = method_names().into_iter().map(Value::from).collect();
    assert!(want.contains(&json!("admin")));

    let list = tenant.tools_list().await.expect("tools/list");
    let claim_tools: Vec<&Value> =
        list["tools"].as_array().unwrap().iter().filter(|t| t["name"].as_str().is_some_and(|n| n.starts_with("host.claim."))).collect();
    assert!(!claim_tools.is_empty());
    for t in claim_tools {
        assert_eq!(t["inputSchema"]["properties"]["method"]["enum"], json!(want), "{}", t["name"]);
    }

    let doc: Value = reqwest::get(format!("{}/claim/methods.json", server.base_url)).await.unwrap().json().await.unwrap();
    let tools: Vec<&str> = doc["methods"].as_array().unwrap().iter().map(|m| m["tool"].as_str().unwrap()).collect();
    assert_eq!(tools, agent_startable().map(|(_, t)| *t).collect::<Vec<_>>());
    assert!(tools.iter().all(|t| t.starts_with("host.")), "{tools:?}");

    let quickstart = extract_structured(&tenant.tools_call("host.quickstart", json!({"kind": "echo"})).await.expect("quickstart"));
    let steps: Vec<&str> = quickstart["next"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["method"].is_string())
        .map(|s| s["call"].as_str().unwrap())
        .collect();
    assert_eq!(steps, tools, "quickstart lists exactly the host.-prefixed table entries");
}

#[tokio::test]
async fn emslp_ac04_every_table_entry_has_a_dispatch_arm() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "emslp ac4 arms").await;
    let tenant = McpClient::with_bearer(&server.base_url, &key);
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    for (method, tool) in VERIFICATION_METHODS {
        let client = if tool.starts_with("admin.") { &admin } else { &tenant };
        let args = json!({"domain": "emslp-ac04.example.dev", "tenant": ns, "email": "not-an-email"});
        // A real tenant and a malformed email: the admin arm answers args_invalid and writes
        // nothing, while a missing arm answers tool_not_found.
        if let Err(e) = client.tools_call(tool, args).await {
            assert_ne!(e.error_code.as_deref(), Some("tool_not_found"), "{method}: {tool} has a table entry but no dispatch arm");
        }
    }
}

#[tokio::test]
async fn emslp_ac04_the_admin_verb_is_listed_for_the_admin_key_only() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "emslp ac4 visibility").await;
    let tenant_names = tool_names(&McpClient::with_bearer(&server.base_url, &key).tools_list().await.expect("tenant list"));
    let anon_names = tool_names(&McpClient::new(&server.base_url).tools_list().await.expect("anon list"));
    let admin_names = tool_names(&McpClient::with_bearer(&server.base_url, ADMIN_KEY).tools_list().await.expect("admin list"));

    // An alias would be this name with its separators changed, so match on the stem.
    let stem = |n: &str| n.replace(['.', '_'], "").contains("tenantverifyowner");
    for (who, names) in [("tenant key", &tenant_names), ("no key", &anon_names)] {
        assert!(!names.iter().any(|n| stem(n)), "{who} must not see the admin verb or an alias: {:?}", names.iter().filter(|n| stem(n)).collect::<Vec<_>>());
    }
    assert!(admin_names.iter().any(|n| n == ADMIN_TOOL), "the admin key's list carries {ADMIN_TOOL}");
}
