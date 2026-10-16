//! PRD-mcphost-email-inbound-route
//! AC1 -- Given the route registered, When `POST /email/inbound/no-such-tenant`
//! is sent with the probe payload from a fleet IP, Then 404 with the standard
//! error envelope and `code: "tenant_not_found"`, and nothing is written.

use crate::common::TestServer;
use serde_json::json;

#[tokio::test]
async fn emin_ac01_unknown_namespace_is_404_tenant_not_found() {
    // 127.0.0.1 is the test client's source address, so it is a fleet IP here.
    let server = TestServer::start_with_signup_rate_limit_and_fleet_ips(100, "127.0.0.1").await;
    let payload = json!({
        "provider_message_id": "probe-ac1",
        "from": "probe@example.com",
        "subject": "email-probe attach lines.csv",
        "parts": [{"filename": "lines.csv", "content_type": "text/csv", "content_base64": "YSxiCjEsMgo=", "inline": false}],
    });
    let resp = reqwest::Client::new()
        .post(format!("{}/email/inbound/no-such-tenant", server.base_url))
        .json(&payload)
        .send()
        .await
        .expect("POST");
    assert_eq!(resp.status(), 404);
    let body: serde_json::Value = resp.json().await.expect("json envelope");
    assert_eq!(body["error_code"], json!("tenant_not_found"));
    assert_eq!(body["error"]["code"], json!("tenant_not_found"));
    assert!(body["error"]["message"].is_string(), "{body}");
}
