//! PRD-mcphost-email-inbound-route
//! AC4 -- Given a non-JSON body or JSON without `provider_message_id`, When
//! posted by an authorized caller, Then 400 `invalid_payload` and nothing is
//! stored.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

#[tokio::test]
async fn emin_ac04_non_json_or_missing_provider_message_id_is_400() {
    let server = TestServer::start_with_signup_rate_limit_and_fleet_ips(100, "127.0.0.1").await;
    let (ns, key) = signup(&server.base_url, "Emin AC4").await;
    let tenant = server.state.db.find_tenant_by_namespace(ns.clone()).await.expect("lookup").expect("tenant");
    let url = format!("{}/email/inbound/{ns}", server.base_url);
    let http = reqwest::Client::new();

    let bodies = [
        ("not json", "this is not json".to_string()),
        ("empty body", String::new()),
        ("no provider_message_id", json!({"from": "a@example.com", "parts": []}).to_string()),
        ("blank provider_message_id", json!({"provider_message_id": ""}).to_string()),
        ("json array", "[1, 2]".to_string()),
    ];
    for (label, body) in bodies {
        let resp = http.post(&url).header("content-type", "application/json").body(body).send().await.expect("POST");
        assert_eq!(resp.status(), 400, "{label}");
        let envelope: Value = resp.json().await.expect("envelope");
        assert_eq!(envelope["error_code"], json!("invalid_payload"), "{label}");
        assert_eq!(envelope["error"]["code"], json!("invalid_payload"), "{label}");
    }

    assert_eq!(server.state.db.email_blob_count(tenant.id, None).await.unwrap(), 0);
    let client = McpClient::with_bearer(&server.base_url, &key);
    let inbox = extract_structured(&client.tools_call("host.msg.inbox", json!({})).await.expect("inbox"));
    assert!(inbox["messages"].as_array().expect("messages").is_empty(), "{inbox}");
}
