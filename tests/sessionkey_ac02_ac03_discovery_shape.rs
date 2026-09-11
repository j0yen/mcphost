//! PRD-mcphost-session-key
//! AC2 — Given a server with a tenant that has published a tool, When an
//! anonymous client calls `tools/list`, Then no namespaced tool such as
//! `t_xxxxxxxx.hello` appears in the result.
//! AC3 — Given the anonymous tool list, When any `host.*` descriptor is
//! inspected, Then its input schema declares an optional string property
//! `tenant_key` and does not list it in `required`.

mod common;
use common::{McpClient, TestServer, signup};

#[tokio::test]
async fn anonymous_list_never_shows_a_namespaced_tool() {
    let server = TestServer::start().await;

    let (ns, key) = signup(&server.base_url, "Publisher").await;
    let publisher = McpClient::with_bearer(&server.base_url, &key);
    publisher
        .tools_call(
            "host.tool_publish",
            serde_json::json!({"name": "hello", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");

    let anon = McpClient::new(&server.base_url);
    let tools = anon.tools_list().await.expect("anonymous tools/list");
    let names: Vec<&str> = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();

    assert!(
        !names.iter().any(|n| n.starts_with(&format!("{ns}."))),
        "anonymous tools/list must never show a namespaced tool: {names:?}"
    );
}

#[tokio::test]
async fn every_host_star_descriptor_declares_an_optional_tenant_key() {
    let server = TestServer::start().await;
    let anon = McpClient::new(&server.base_url);
    let tools = anon.tools_list().await.expect("anonymous tools/list");
    let entries = tools["tools"].as_array().expect("tools array");

    let host_tools: Vec<_> = entries
        .iter()
        .filter(|t| t["name"].as_str().unwrap().starts_with("host."))
        .collect();
    assert_eq!(
        host_tools.len(),
        43,
        "the thirteen host.* control tools (incl. host.quickstart, host.tool_run, and \
         host.bridge_test) plus host.tool_call plus the nine host.state.* tools \
         (PRD-mcphost-tenant-state) plus the eight host.tool_share/host.tool_unshare/ \
         host.group.*/host.catalog.* tools (PRD-mcphost-sharing) plus the five \
         host.runs.* tools (PRD-mcphost-runs-and-jobs) plus the seven \
         host.trigger.* tools (PRD-mcphost-schedules): {entries:?}"
    );

    for tool in host_tools {
        let name = tool["name"].as_str().unwrap();
        let props = &tool["inputSchema"]["properties"];
        assert_eq!(
            props["tenant_key"]["type"].as_str(),
            Some("string"),
            "{name}'s inputSchema must declare a string tenant_key property: {tool}"
        );
        let required = tool["inputSchema"]["required"]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
            .unwrap_or_default();
        assert!(
            !required.contains(&"tenant_key"),
            "{name} must not require tenant_key: {tool}"
        );
    }
}
