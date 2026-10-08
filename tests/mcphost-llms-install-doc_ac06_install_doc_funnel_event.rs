//! AC6 (PRD-mcphost-llms-install-doc) — Given a GET `/llms-install.md`
//! from a non-fleet IP, When handled, Then a funnel event `install_doc`
//! with `origin=human` is recorded; a fleet IP records `origin=fleet`.

use crate::common;
use common::TestServer;

const FLEET_IP: &str = "46.225.110.44";
const HUMAN_IP: &str = "8.8.8.8";

async fn fetch(base_url: &str, x_forwarded_for: &str) {
    let response = reqwest::Client::new()
        .get(format!("{base_url}/llms-install.md"))
        .header("x-forwarded-for", x_forwarded_for)
        .send()
        .await
        .expect("GET /llms-install.md");
    assert_eq!(response.status(), 200);
}

#[tokio::test]
async fn fetches_record_install_doc_events_by_ip_class() {
    let server = TestServer::start_with_signup_rate_limit_and_fleet_ips(100, FLEET_IP).await;
    let counts = || async { server.state.db.funnel_event_origin_counts("install_doc").await.expect("counts") };
    assert!(counts().await.is_empty(), "no events before any fetch");

    fetch(&server.base_url, HUMAN_IP).await;
    assert_eq!(counts().await, vec![("human".to_string(), 1)]);

    fetch(&server.base_url, FLEET_IP).await;
    assert_eq!(counts().await, vec![("fleet".to_string(), 1), ("human".to_string(), 1)]);

    fetch(&server.base_url, HUMAN_IP).await;
    assert_eq!(counts().await, vec![("fleet".to_string(), 1), ("human".to_string(), 2)]);
}
