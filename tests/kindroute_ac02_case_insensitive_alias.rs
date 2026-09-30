//! PRD-mcphost-unknown-kind-routes-to-recipe
//! AC2 (P0) -- Given the same `host.quickstart` call with `kind: "EVENT"`,
//! When the server responds, Then it resolves identically to `kind:
//! "event"` (case-insensitive).

use crate::common;
use common::{TempDataDir, TestServer, all_kinds_registry, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn quickstart_event_alias_is_case_insensitive() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(all_kinds_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Kindroute AC2 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let lower = extract_structured(
        &client
            .tools_call("host.quickstart", json!({"kind": "event"}))
            .await
            .expect("quickstart lowercase"),
    );
    let upper = extract_structured(
        &client
            .tools_call("host.quickstart", json!({"kind": "EVENT"}))
            .await
            .expect("quickstart uppercase"),
    );

    assert_eq!(upper["kind"], json!("http"));
    assert_eq!(upper["kind"], lower["kind"]);
    assert_eq!(
        upper["resolved_from"].as_str().map(str::to_ascii_lowercase),
        Some("event".to_string())
    );
    assert_eq!(upper["recipe"]["name"], lower["recipe"]["name"]);
    assert_eq!(upper["recipe"]["steps"], lower["recipe"]["steps"]);
}
