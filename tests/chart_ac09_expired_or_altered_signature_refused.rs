//! PRD-mcphost-chart-in-a-minute
//! AC9 — Given a share URL past its expiry or with an altered signature,
//! When fetched, Then 410 or 403 respectively and no chart body.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use reqwest::StatusCode;

use crate::chart_fixture;

#[tokio::test]
async fn expired_signed_url_is_410() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Chart AC9a Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    chart_fixture::seed_expenses(&client).await;

    let result = client
        .tools_call(
            "host.table.chart",
            serde_json::json!({
                "sql": "SELECT category, SUM(amount) AS total FROM expenses GROUP BY category",
                "share": true,
            }),
        )
        .await
        .expect("host.table.chart");
    let chart = extract_structured(&result);
    let chart_id = chart["chart_id"].as_str().expect("chart_id is a string");

    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("find tenant").expect("tenant exists");
    let past_expiry = mcphost::state::now_unix() - 10;
    let expired_url = mcphost::chart::signed_chart_url(&server.base_url, &tenant.key_hash, chart_id, past_expiry);

    let resp = reqwest::get(&expired_url).await.expect("GET expired url");
    assert_eq!(resp.status(), StatusCode::GONE, "url: {expired_url}");
    let body = resp.text().await.expect("body");
    assert!(!body.contains("vega-embed"), "an expired link must not leak the chart body: {body}");
}

#[tokio::test]
async fn altered_signature_is_403() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Chart AC9b Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    chart_fixture::seed_expenses(&client).await;

    let result = client
        .tools_call(
            "host.table.chart",
            serde_json::json!({
                "sql": "SELECT category, SUM(amount) AS total FROM expenses GROUP BY category",
                "share": true,
            }),
        )
        .await
        .expect("host.table.chart");
    let chart = extract_structured(&result);
    let share_url = chart["share_url"].as_str().expect("share_url is a string").to_string();

    // Flip one hex digit of the signature -- same "altered, not just
    // missing" shape the export AC2 precedent proves elsewhere.
    let tampered_url = if share_url.ends_with('0') {
        format!("{}1", &share_url[..share_url.len() - 1])
    } else {
        format!("{}0", &share_url[..share_url.len() - 1])
    };
    assert_ne!(tampered_url, share_url);

    let resp = reqwest::get(&tampered_url).await.expect("GET tampered url");
    assert_eq!(resp.status(), StatusCode::FORBIDDEN, "url: {tampered_url}");
    let body = resp.text().await.expect("body");
    assert!(!body.contains("vega-embed"), "a bad signature must not leak the chart body: {body}");
}
