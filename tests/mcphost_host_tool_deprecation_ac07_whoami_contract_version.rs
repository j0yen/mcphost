//! PRD-mcphost-host-tool-deprecation AC7 — Given any tenant, When
//! `host.whoami` is called, Then `contract_version` names the contract.
//! PRD-mcphost-contract-version-reported R2: it is now the string
//! `1.<sha12>`, matching `^1\.[0-9a-f]{12}$`, not the bare integer 1.

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

    let version = structured["contract_version"]
        .as_str()
        .unwrap_or_else(|| panic!("contract_version must be a string: {structured}"));
    let prefix = format!("{}.", mcphost::api_contract::CONTRACT_VERSION);
    let short = version
        .strip_prefix(&prefix)
        .unwrap_or_else(|| panic!("must start with {prefix}: {version}"));
    assert!(
        prefix == "1."
            && short.len() == 12
            && short.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')),
        "contract_version must match ^1\\.[0-9a-f]{{12}}$: {version}"
    );
}
