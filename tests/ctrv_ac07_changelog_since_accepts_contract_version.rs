//! PRD-mcphost-contract-version-reported AC7 — Given
//! `host.changelog({since: "<contract_version string>"})`, When called, Then
//! it returns the same added/changed lists as `since: "<the release that
//! produced it>"`, and an unknown sha returns `args_invalid` with
//! `data.argument = "since"`. (The changelog's lists are named `additions`,
//! `deprecations` and `removals`.)

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn changelog_since_contract_version_equals_since_its_release_and_unknown_sha_is_args_invalid() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "CTRV AC7").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let whoami = extract_structured(&client.tools_call("host.whoami", json!({})).await.unwrap());
    let version = whoami["contract_version"].as_str().unwrap().to_string();

    let by_sha = extract_structured(
        &client
            .tools_call("host.changelog", json!({"since": version}))
            .await
            .expect("changelog by contract_version"),
    );
    let by_release = extract_structured(
        &client
            .tools_call("host.changelog", json!({"since": env!("CARGO_PKG_VERSION")}))
            .await
            .expect("changelog by release"),
    );
    for list in ["additions", "deprecations", "removals"] {
        assert_eq!(by_sha[list], by_release[list], "{list} must match");
    }

    let unknown = client
        .tools_call("host.changelog", json!({"since": "1.000000000000"}))
        .await
        .expect_err("an unknown contract sha must be rejected");
    assert_eq!(unknown.error_code.as_deref(), Some("args_invalid"), "{unknown:?}");
    assert_eq!(unknown.data["argument"], "since", "{unknown:?}");
}
