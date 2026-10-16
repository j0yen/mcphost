//! PRD-mcphost-email-inbound-route
//! AC2 -- Given a known namespace, When the probe payload is posted from a
//! non-fleet IP without `X-Mcphost-Inbound-Secret`, Then 401
//! `inbound_unauthorized` and nothing is stored; Given the header equals env
//! `MCPHOST_EMAIL_INBOUND_SECRET`, Then the request proceeds (AC3).

use crate::common::{EnvGuard, McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

const SECRET: &str = "emin-test-secret";

fn payload() -> Value {
    json!({
        "provider_message_id": "probe-ac2",
        "from": "probe@example.com",
        "subject": "email-probe attach lines.csv",
        "parts": [{"filename": "lines.csv", "content_type": "text/csv", "content_base64": "YSxiCjEsMgo=", "inline": false}],
    })
}

#[tokio::test]
async fn emin_ac02_non_fleet_caller_needs_the_secret_header() {
    let _secret = EnvGuard::set("MCPHOST_EMAIL_INBOUND_SECRET", SECRET).await;
    // Empty fleet list: the loopback test client is a non-fleet caller.
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Emin AC2").await;
    let tenant = server.state.db.find_tenant_by_namespace(ns.clone()).await.expect("lookup").expect("tenant");
    let url = format!("{}/email/inbound/{ns}", server.base_url);
    let http = reqwest::Client::new();

    for (label, req) in [
        ("no header", http.post(&url)),
        ("wrong header", http.post(&url).header("X-Mcphost-Inbound-Secret", "nope")),
    ] {
        let resp = req.json(&payload()).send().await.expect("POST");
        assert_eq!(resp.status(), 401, "{label}");
        let body: Value = resp.json().await.expect("envelope");
        assert_eq!(body["error_code"], json!("inbound_unauthorized"), "{label}");
        assert_eq!(body["error"]["code"], json!("inbound_unauthorized"), "{label}");
    }
    assert_eq!(server.state.db.email_blob_count(tenant.id, None).await.unwrap(), 0);
    let client = McpClient::with_bearer(&server.base_url, &key);
    let inbox = extract_structured(&client.tools_call("host.msg.inbox", json!({})).await.expect("inbox"));
    assert!(inbox["messages"].as_array().expect("messages").is_empty(), "{inbox}");

    let resp = http
        .post(&url)
        .header("X-Mcphost-Inbound-Secret", SECRET)
        .json(&payload())
        .send()
        .await
        .expect("POST with secret");
    assert_eq!(resp.status(), 200, "the right secret passes authorization (AC3)");
}
