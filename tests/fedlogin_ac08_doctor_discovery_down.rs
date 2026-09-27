//! PRD-mcphost-federated-end-user-login
//! AC8 (P1) — Given the provider's discovery endpoint down, When
//! `host.oauth.doctor` runs, Then the discovery check reads `fail` with
//! the fix text and the other checks still report.

use crate::common;
use crate::federation;

use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

#[tokio::test]
async fn discovery_down_fails_with_fix_text_other_checks_still_report() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Acme").await;
    let key_client = McpClient::with_bearer(&server.base_url, &key);

    let provider = federation::start().await;
    key_client
        .tools_call(
            "host.oauth.provider_set",
            json!({"issuer": provider.issuer(), "client_id": "acme-client", "client_secret": "acme-secret"}),
        )
        .await
        .expect("provider_set must succeed");

    federation::take_discovery_down(&provider).await;

    let doctor_result =
        extract_structured(&key_client.tools_call("host.oauth.doctor", json!({})).await.expect("doctor must succeed"));
    let checks = doctor_result["checks"].as_array().expect("checks array");
    assert_eq!(checks.len(), 5, "every check must still report: {checks:?}");

    let by_name = |name: &str| -> &Value { checks.iter().find(|c| c["name"] == json!(name)).expect("check present") };

    let discovery = by_name("discovery");
    assert_eq!(discovery["status"], json!("fail"), "{discovery}");
    assert!(discovery["fix"].as_str().is_some_and(|s| !s.is_empty()), "fix text must be present: {discovery}");

    for name in ["jwks", "metadata", "callback_registered", "dry_authorize"] {
        let check = by_name(name);
        assert!(check.get("status").is_some(), "check '{name}' must still report a status: {check}");
    }
}
