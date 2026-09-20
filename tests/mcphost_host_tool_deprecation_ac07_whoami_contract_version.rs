//! PRD-mcphost-host-tool-deprecation AC7 — Given any tenant, When
//! `host.whoami` is called, Then `contract_version` is 1.

use crate::common;
use common::{TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn whoami_reports_contract_version_1() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC7 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let result = client
        .tools_call("host.whoami", json!({}))
        .await
        .expect("host.whoami must succeed");
    let structured = common::extract_structured(&result);

    assert_eq!(
        structured["contract_version"], 1,
        "host.whoami must report contract_version 1: {structured}"
    );
    assert_eq!(
        structured["contract_version"],
        mcphost::api_contract::CONTRACT_VERSION,
        "must stay in sync with the contract dump's own contract_version"
    );
}
