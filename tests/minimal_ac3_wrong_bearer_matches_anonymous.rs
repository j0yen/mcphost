//! PRD-mcphost-healthz-minimal
//! AC3 — Given a wrong bearer, When the request is handled, Then the
//! response is byte-identical to the anonymous response.

mod common;
use common::TestServer;

#[tokio::test]
async fn wrong_bearer_is_byte_identical_to_anonymous() {
    let server = TestServer::start().await;

    let anon = reqwest::get(format!("{}/healthz", server.base_url))
        .await
        .expect("GET /healthz (anonymous)");
    let anon_status = anon.status();
    let anon_bytes = anon.bytes().await.expect("read anonymous body");

    let wrong = reqwest::Client::new()
        .get(format!("{}/healthz", server.base_url))
        .bearer_auth("not-the-admin-key")
        .send()
        .await
        .expect("GET /healthz (wrong bearer)");
    let wrong_status = wrong.status();
    let wrong_bytes = wrong.bytes().await.expect("read wrong-bearer body");

    assert_eq!(anon_status, wrong_status, "status must match too");
    assert_eq!(
        anon_bytes, wrong_bytes,
        "a wrong bearer must be byte-identical to no bearer at all -- a probing \
         client must learn nothing about the key format from the difference"
    );
}
