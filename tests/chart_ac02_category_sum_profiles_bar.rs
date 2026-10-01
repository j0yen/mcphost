//! PRD-mcphost-chart-in-a-minute
//! AC2 — Given the 1,000-row fixture, When `host.table.chart` runs `SELECT
//! category, SUM(amount) AS total FROM expenses GROUP BY category`, Then
//! the profile has one nominal dimension and one quantitative measure, the
//! recommendation mark is a bar, and the spec's `data.values` holds five
//! rows.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};

use crate::chart_fixture;

#[tokio::test]
async fn category_sum_profiles_one_dimension_one_measure_and_recommends_bar() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Chart AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    chart_fixture::seed_expenses(&client).await;

    let result = client
        .tools_call(
            "host.table.chart",
            serde_json::json!({"sql": "SELECT category, SUM(amount) AS total FROM expenses GROUP BY category"}),
        )
        .await
        .expect("host.table.chart");
    let chart = extract_structured(&result);

    assert_eq!(chart["schema"], "chart.v1", "chart: {chart}");
    assert_eq!(chart["row_count"], 5, "chart: {chart}");

    let columns = chart["profile"]["columns"].as_array().expect("profile.columns array");
    assert_eq!(columns.len(), 2, "profile: {}", chart["profile"]);
    let dimensions: Vec<_> = columns.iter().filter(|c| c["role"] == "dimension").collect();
    let measures: Vec<_> = columns.iter().filter(|c| c["role"] == "measure").collect();
    assert_eq!(dimensions.len(), 1, "profile: {}", chart["profile"]);
    assert_eq!(measures.len(), 1, "profile: {}", chart["profile"]);
    assert_eq!(dimensions[0]["name"], "category", "profile: {}", chart["profile"]);
    assert_eq!(dimensions[0]["data_type"], "nominal", "profile: {}", chart["profile"]);
    assert_eq!(measures[0]["name"], "total", "profile: {}", chart["profile"]);
    assert_eq!(measures[0]["data_type"], "quantitative", "profile: {}", chart["profile"]);
    assert_eq!(chart["profile"]["measure_count"], 1, "profile: {}", chart["profile"]);
    assert_eq!(chart["profile"]["dimension_count"], 1, "profile: {}", chart["profile"]);

    assert_eq!(chart["recommendation"]["mark"], "bar", "recommendation: {}", chart["recommendation"]);

    let data_values = chart["vega_lite"]["data"]["values"].as_array().expect("data.values array");
    assert_eq!(data_values.len(), 5, "vega_lite: {}", chart["vega_lite"]);
}
