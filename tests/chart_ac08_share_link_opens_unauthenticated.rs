//! PRD-mcphost-chart-in-a-minute
//! AC8 — Given `share: true`, When the returned `share_url` is fetched
//! without any auth header, Then 200 with HTML containing the spec inline
//! and the caption headline, and no tenant id or key appears in the body.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use reqwest::StatusCode;

use crate::chart_fixture;

#[tokio::test]
async fn share_url_opens_with_spec_and_caption_and_no_tenant_identifiers() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Chart AC8 Tenant").await;
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

    // Plain, unauthenticated GET -- no bearer, no tenant_key argument.
    let resp = reqwest::get(&share_url).await.expect("GET share_url");
    assert_eq!(resp.status(), StatusCode::OK, "url: {share_url}");
    let body = resp.text().await.expect("share page body");

    // The spec is inline -- its own data.values row content shows up in
    // the page, not a call back to mcphost.
    assert!(body.contains("\"category\""), "body must inline the spec's own fields: {body}");
    assert!(body.contains("\"total\""), "body must inline the spec's own fields: {body}");

    let headline = chart["caption"]["headline"].as_str().expect("caption.headline is a string");
    assert!(body.contains(headline), "body must show the caption headline {headline:?}: {body}");

    // No tenant identifiers anywhere in the markup.
    assert!(!body.contains(&ns), "body must not leak the tenant namespace {ns}: {body}");
    assert!(!body.contains(&key), "body must not leak the tenant key: {body}");
    assert!(
        !body.to_lowercase().contains("key_hash"),
        "body must not leak key_hash: {body}"
    );
}
