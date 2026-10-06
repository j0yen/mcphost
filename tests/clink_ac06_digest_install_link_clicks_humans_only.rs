//! AC6 (PRD-mcphost-client-install-links) — Given install-link events of
//! mixed origins, When the digest runs, Then it lists clicks per client
//! for humans only.

use crate::common;
use common::TestServer;
use serde_json::json;

const FLEET_IP: &str = "46.225.110.44";
const HUMAN_IP: &str = "8.8.8.8";

async fn connect_go(base_url: &str, client: &str, x_forwarded_for: &str) {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("client")
        .get(format!("{base_url}/connect/go/{client}"))
        .header("x-forwarded-for", x_forwarded_for)
        .send()
        .await
        .expect("GET /connect/go/{client}");
}

#[tokio::test]
async fn digest_counts_install_link_clicks_per_client_for_humans_only() {
    let server = TestServer::start_with_signup_rate_limit_and_fleet_ips(100, FLEET_IP).await;

    // Two human cursor clicks, one human vscode click, one fleet cursor
    // click -- the fleet click must not count toward the digest at all.
    connect_go(&server.base_url, "cursor", HUMAN_IP).await;
    connect_go(&server.base_url, "cursor", HUMAN_IP).await;
    connect_go(&server.base_url, "vscode", HUMAN_IP).await;
    connect_go(&server.base_url, "cursor", FLEET_IP).await;

    let report = mcphost::admin::funnel(&server.state, &json!({})).await.expect("admin.funnel");
    let clicks = &report["install_link_clicks"];

    assert_eq!(clicks["cursor"], json!(2), "humans-only cursor count: {report}");
    assert_eq!(clicks["vscode"], json!(1), "humans-only vscode count: {report}");
    assert!(
        clicks.get("claude-code").is_none(),
        "a client with zero clicks must not appear at all: {report}"
    );
}
