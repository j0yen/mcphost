//! PRD-mcphost-key-scoped-to-tools
//! AC2 (P0) — Given a sub-key for `["aa"]`, When `tools/list` runs, Then
//! exactly `aa` plus the base set (`host.whoami`, `host.quickstart`,
//! `host.tool.reach`, the last only once that tool exists) is returned; When
//! `tools/call host.secret.list` runs, Then `permission_denied` with
//! `data.allowed_tools == ["aa"]` and `data.key_label`.

use crate::common;
use common::{McpClient, TestServer, publish, signup};
use serde_json::json;

#[tokio::test]
async fn sub_key_list_is_allowlist_plus_base_and_refusal_names_boundary() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Owner").await;
    let owner = McpClient::with_bearer(&server.base_url, &key);
    publish(&owner, "aa", "echo", json!({"schema": {"type": "object"}})).await;
    publish(&owner, "bb", "echo", json!({"schema": {"type": "object"}})).await;
    let tenant = server.state.db.find_tenant_by_namespace(ns.clone()).await.unwrap().unwrap();

    let sub_key = format!("mk_{}", mcphost::auth::generate_key());
    server
        .state
        .db
        .insert_sub_key(tenant.id, mcphost::auth::hash_key(&sub_key), "ci".into(), vec!["aa".into()], 1, i64::MAX / 2)
        .await
        .unwrap();
    let client = McpClient::with_bearer(&server.base_url, &sub_key);

    let listed = client.tools_list().await.expect("tools/list");
    let mut names: Vec<String> = listed["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    names.sort();
    // Generated contract: the base set is the shared const, filtered to tools
    // that exist.
    let all_host: Vec<String> = owner.tools_list().await.unwrap()["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    let mut expected: Vec<String> = mcphost::handler::SUB_KEY_BASE_TOOLS
        .iter()
        .filter(|n| all_host.iter().any(|h| h == *n))
        .map(|n| n.to_string())
        .collect();
    expected.push(format!("{ns}.aa"));
    expected.sort();
    assert_eq!(names, expected);
    assert!(names.contains(&"host.whoami".to_string()) && names.contains(&"host.quickstart".to_string()));

    // The always-refused set names only real host tools.
    for n in mcphost::handler::SUB_KEY_REFUSED_TOOLS {
        assert!(all_host.iter().any(|h| h == n), "{n} is not a host tool");
    }

    let err = client
        .tools_call("host.secret.list", json!({}))
        .await
        .expect_err("host.secret.list must be refused");
    assert_eq!(err.error_code.as_deref(), Some("permission_denied"));
    assert_eq!(err.data["allowed_tools"], json!(["aa"]));
    assert_eq!(err.data["key_label"], json!("ci"));

    // A tool outside the allowlist is refused too.
    let err = client.tools_call(&format!("{ns}.bb"), json!({})).await.expect_err("bb refused");
    assert_eq!(err.error_code.as_deref(), Some("permission_denied"));
}
