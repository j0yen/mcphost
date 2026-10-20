//! PRD-mcphost-resume-tool AC5 -- Given an anonymous session on bare `/mcp`,
//! When `host.resume` is the first call, Then the response carries
//! `onboarding` and `next_call.tool = "host.quickstart"`, `signup_events`
//! gains no row, and `tenants` count is unchanged.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use mcphost::resume::ANON_SAFE_TOOLS;
use serde_json::json;

fn count(server: &TestServer, table: &str) -> i64 {
    let conn = rusqlite::Connection::open(server.state.db.data_dir().join("mcphost.db")).unwrap();
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0)).unwrap()
}

#[tokio::test]
async fn host_resume_as_the_first_anonymous_call_returns_onboarding_and_signs_nobody_up() {
    let server = TestServer::start().await;
    let (tenants, signups) = (count(&server, "tenants"), count(&server, "signup_events"));

    let client = McpClient::new(&server.base_url);
    let result = client.tools_call("host.resume", json!({})).await.expect("anonymous host.resume");
    let out = extract_structured(&result);

    assert!(out.get("onboarding").is_some_and(|o| o.is_object()), "{out}");
    assert_eq!(out["next_call"]["tool"], "host.quickstart", "{out}");
    assert!(out.get("tools").is_none(), "an anonymous resume has no tenant sections: {out}");
    assert_eq!(count(&server, "tenants"), tenants, "no tenant was created");
    assert_eq!(count(&server, "signup_events"), signups, "no signup event was written");
}

/// The one const behind both lists: the anonymous starter `tools/list`
/// contains every member, and an anonymous call to any member never
/// implicit-signs-up.
#[tokio::test]
async fn the_starter_list_contains_every_anon_safe_tool_and_the_gate_skips_each() {
    let server = TestServer::start().await;
    let listed = McpClient::new(&server.base_url).tools_list().await.expect("anonymous tools/list");
    let names: Vec<&str> = listed["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(ANON_SAFE_TOOLS.contains(&"host.resume"));
    for tool in ANON_SAFE_TOOLS {
        assert!(names.contains(tool), "starter list is missing {tool}: {names:?}");
    }

    let tenants = count(&server, "tenants");
    for tool in ANON_SAFE_TOOLS {
        // Each call may well be refused for its arguments; none may sign anyone up.
        let _ = McpClient::new(&server.base_url).tools_call(tool, json!({})).await;
        assert_eq!(count(&server, "tenants"), tenants, "{tool} created a tenant");
    }
}
