//! PRD-mcphost-chart-in-a-minute
//! AC6 — Given `SELECT COUNT(*) FROM expenses`, When chart runs, Then mark
//! is `BigNumber` and the caption headline states the count.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};

use crate::chart_fixture;

#[tokio::test]
async fn count_star_recommends_bignumber_and_headline_states_the_count() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Chart AC6 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    chart_fixture::seed_expenses(&client).await;

    let result = client
        .tools_call("host.table.chart", serde_json::json!({"sql": "SELECT COUNT(*) FROM expenses"}))
        .await
        .expect("host.table.chart");
    let chart = extract_structured(&result);

    assert_eq!(chart["row_count"], 1, "chart: {chart}");
    assert_eq!(chart["recommendation"]["mark"], "bignumber", "recommendation: {}", chart["recommendation"]);

    let headline = chart["caption"]["headline"].as_str().expect("caption.headline is a string");
    assert!(
        headline.contains(&chart_fixture::expenses_rows().len().to_string()),
        "headline must state the row count 1000: {headline}"
    );
}
