//! PRD-mcphost-tool-reach-manifest-v2
//! AC2 (P0) -- Given an http tool whose url host is `{{ args.host }}`, When it
//! is published and read, Then `dynamic_hosts` is `true`, `hosts` holds only
//! literal hosts, and this PRD does not refuse the publish.

use crate::common;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
use serde_json::{Value, json};

async fn publish_and_reach(client: &McpClient, name: &str, url: &str) -> Value {
    client
        .tools_call(
            "host.tool.publish",
            json!({"name": name, "kind": "http", "spec": {"method": "GET", "url": url}}),
        )
        .await
        .unwrap_or_else(|e| panic!("publish {name} ({url}) must not be refused: {} {}", e.code, e.message));
    let result = client
        .tools_call("host.tool.reach", json!({"name": name}))
        .await
        .unwrap_or_else(|e| panic!("host.tool.reach {name}: {} {}", e.code, e.message));
    extract_structured(&result)
}

#[tokio::test]
async fn trm_ac02_a_templated_host_sets_dynamic_hosts_and_publishes() {
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "TRM AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    // The whole host is a template.
    let whole = publish_and_reach(&client, "any_host", "https://{{ args.host }}/v1/items").await;
    assert_eq!(whole["tool_version"], json!(1));
    assert_eq!(whole["reach"]["dynamic_hosts"], json!(true), "{whole}");
    assert_eq!(whole["reach"]["hosts"], json!([]), "a templated host is not a literal host: {whole}");

    // Part of the host is a template: the literal tail is not a host of its own.
    let part = publish_and_reach(&client, "any_subdomain", "https://{{ args.sub }}.example.com/v1").await;
    assert_eq!(part["reach"]["dynamic_hosts"], json!(true), "{part}");
    assert_eq!(part["reach"]["hosts"], json!([]), "{part}");

    // A literal host with a templated path stays literal and static.
    let literal = publish_and_reach(&client, "fixed_host", "https://api.example.com/v1/{{ args.id }}").await;
    assert_eq!(literal["reach"]["dynamic_hosts"], json!(false), "{literal}");
    assert_eq!(literal["reach"]["hosts"], json!(["api.example.com"]), "{literal}");
}
