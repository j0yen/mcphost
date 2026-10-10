//! PRD-mcphost-contract-version-reported AC2 — Given a keyed tenant, an
//! admin bearer and an anonymous implicit-signup connection, When each
//! calls `host.whoami`, Then `contract_version` matches `^1\.[0-9a-f]{12}$`,
//! `contract_sha` is 64 hex chars, and all three values are identical.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use serde_json::json;

fn is_hex(s: &str) -> bool {
    s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

async fn whoami(client: &McpClient) -> serde_json::Value {
    extract_structured(
        &client
            .tools_call("host.whoami", json!({}))
            .await
            .expect("host.whoami must succeed"),
    )
}

#[tokio::test]
async fn whoami_contract_version_and_sha_are_identical_for_tenant_admin_and_anonymous() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "CTRV AC2").await;

    let tenant = whoami(&McpClient::with_bearer(&server.base_url, &key)).await;
    let admin = whoami(&McpClient::with_bearer(&server.base_url, ADMIN_KEY)).await;
    let anon = whoami(&McpClient::new(&server.base_url)).await;

    for (label, v) in [("tenant", &tenant), ("admin", &admin), ("anonymous", &anon)] {
        let version = v["contract_version"].as_str().unwrap_or_else(|| {
            panic!("{label}: contract_version must be a string: {v}");
        });
        let sha = v["contract_sha"]
            .as_str()
            .unwrap_or_else(|| panic!("{label}: contract_sha must be a string: {v}"));
        let (major, short) = version
            .split_once('.')
            .unwrap_or_else(|| panic!("{label}: no '.' in {version}"));
        assert_eq!(major, "1", "{label}: {version}");
        assert!(short.len() == 12 && is_hex(short), "{label}: {version}");
        assert!(sha.len() == 64 && is_hex(sha), "{label}: sha {sha}");
        assert_eq!(short, &sha[..12], "{label}: version must be 1.<sha12>");
    }
    assert_eq!(tenant["contract_version"], admin["contract_version"]);
    assert_eq!(tenant["contract_version"], anon["contract_version"]);
    assert_eq!(tenant["contract_sha"], admin["contract_sha"]);
    assert_eq!(tenant["contract_sha"], anon["contract_sha"]);
    assert_eq!(tenant["contract_sha"], server.state.contract.sha.as_str());
}
