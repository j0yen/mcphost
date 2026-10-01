//! PRD-mcphost-chart-in-a-minute
//! AC5 — Given a query returning zero rows, When chart runs, Then mark is
//! `Table`, `vega_lite.data.values` is empty, and the caption headline is
//! the no-takeaway text.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};

use crate::chart_fixture;

#[tokio::test]
async fn zero_rows_recommends_table_with_empty_spec_and_no_takeaway_caption() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Chart AC5 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    chart_fixture::seed_expenses(&client).await;

    let result = client
        .tools_call(
            "host.table.chart",
            serde_json::json!({"sql": "SELECT category, SUM(amount) AS total FROM expenses WHERE category = 'nonexistent' GROUP BY category"}),
        )
        .await
        .expect("host.table.chart");
    let chart = extract_structured(&result);

    assert_eq!(chart["row_count"], 0, "chart: {chart}");
    assert_eq!(chart["recommendation"]["mark"], "table", "recommendation: {}", chart["recommendation"]);

    let data_values = chart["vega_lite"]["data"]["values"].as_array().expect("data.values array");
    assert!(data_values.is_empty(), "vega_lite: {}", chart["vega_lite"]);

    assert_eq!(
        chart["caption"]["headline"],
        mqo_chart_caption::NO_TAKEAWAY_HEADLINE,
        "caption: {}",
        chart["caption"]
    );
    assert!(
        chart["caption"]["facts"].as_array().expect("facts array").is_empty(),
        "caption: {}",
        chart["caption"]
    );
}
