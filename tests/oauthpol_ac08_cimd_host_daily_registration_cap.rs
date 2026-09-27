//! PRD-mcphost-oauth-client-policy
//! AC8 (P0) — Given 101 registrations from one CIMD host in a day, When
//! the 101st arrives, Then 429 with an audit row.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn cimd_server(redirect_uri: &str) -> (MockServer, String) {
    let server = MockServer::start().await;
    let cimd_url = format!("{}/cimd.json", server.uri());
    Mock::given(method("GET"))
        .and(path("/cimd.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "client_id": cimd_url,
            "client_name": "Prolific Connector",
            "redirect_uris": [redirect_uri],
        })))
        .mount(&server)
        .await;
    (server, cimd_url)
}

fn authorize_url(base_url: &str, client_id: &str, redirect_uri: &str, state: &str) -> String {
    format!(
        "{base_url}/oauth/authorize?response_type=code&client_id={}&redirect_uri={}\
         &code_challenge=dummy-challenge&code_challenge_method=S256&state={state}&scope=mcp&resource={base_url}/mcp",
        urlencoding_stub(client_id),
        urlencoding_stub(redirect_uri),
    )
}

/// Same percent-encoding stub `hostedas_ac02` already uses (no
/// `urlencoding` dependency in this crate).
fn urlencoding_stub(s: &str) -> String {
    s.replace(':', "%3A").replace('/', "%2F")
}

#[tokio::test]
async fn the_101st_cimd_authorize_from_one_host_in_a_day_is_rate_limited() {
    let server = TestServer::start().await;
    let admin_client = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let (_cimd_mock, cimd_client_id) = cimd_server("https://prolific.example/callback").await;

    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();

    for i in 0..100 {
        let resp = http
            .get(authorize_url(&server.base_url, &cimd_client_id, "https://prolific.example/callback", &format!("s{i}")))
            .send()
            .await
            .expect("GET /oauth/authorize");
        assert_eq!(
            resp.status(),
            reqwest::StatusCode::OK,
            "registration {i} (within the 100/day cap) must reach the consent page"
        );
    }

    let resp_101 = http
        .get(authorize_url(&server.base_url, &cimd_client_id, "https://prolific.example/callback", "s100"))
        .send()
        .await
        .expect("GET /oauth/authorize (101st)");
    assert_eq!(resp_101.status(), reqwest::StatusCode::TOO_MANY_REQUESTS, "the 101st registration must be rate limited");
    let body = resp_101.text().await.expect("body");
    assert!(body.contains("too_many_requests"), "body: {body}");

    let stats = admin_client.tools_call("admin.oauth.stats", json!({})).await.expect("admin.oauth.stats");
    let stats_structured = common::extract_structured(&stats);
    assert!(
        stats_structured["refused_cimd_rate_limited"].as_i64().unwrap() >= 1,
        "the rate-limit refusal must be counted with an audit row: {stats_structured:?}"
    );
}
