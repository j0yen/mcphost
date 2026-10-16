//! PRD-mcphost-email-inbound-route
//! AC7 -- Given one accepted and one rejected post in the last hour, When
//! `GET /status.json` runs, Then `email_inbound_received_1h` is 1 and
//! `email_inbound_rejected_1h` is 1.

use crate::common::{TestServer, signup};
use serde_json::{Value, json};

#[tokio::test]
async fn emin_ac07_status_json_counts_one_accepted_and_one_rejected() {
    let server = TestServer::start_with_signup_rate_limit_and_fleet_ips(100, "127.0.0.1").await;
    let (ns, _key) = signup(&server.base_url, "Emin AC7").await;
    let http = reqwest::Client::new();
    let url = format!("{}/email/inbound/{ns}", server.base_url);

    let before: Value = http.get(format!("{}/status.json", server.base_url)).send().await.expect("GET").json().await.expect("json");
    assert_eq!(before["email_inbound_received_1h"], json!(0), "{before}");
    assert_eq!(before["email_inbound_rejected_1h"], json!(0));

    let ok = http.post(&url).json(&json!({"provider_message_id": "prov-emin-ac7", "from": "a@example.com"})).send().await.expect("POST");
    assert_eq!(ok.status(), 200);
    let bad = http.post(&url).body("not json").send().await.expect("POST bad");
    assert_eq!(bad.status(), 400);

    let after: Value = http.get(format!("{}/status.json", server.base_url)).send().await.expect("GET").json().await.expect("json");
    assert_eq!(after["email_inbound_received_1h"], json!(1), "{after}");
    assert_eq!(after["email_inbound_rejected_1h"], json!(1), "{after}");
}
