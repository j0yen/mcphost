//! PRD-mcphost-chart-in-a-minute
//! AC4 — Given `SELECT day, SUM(amount) FROM expenses GROUP BY day`, When
//! chart runs, Then `is_temporal` is true for `day` only if it parses as a
//! date; for the integer `day` column the mark is a bar, and for an
//! ISO-date column in a second fixture table the mark is a line.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};

use crate::chart_fixture;

#[tokio::test]
async fn integer_day_is_not_temporal_and_recommends_bar() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Chart AC4a Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    chart_fixture::seed_expenses(&client).await;

    let result = client
        .tools_call("host.table.chart", serde_json::json!({"sql": "SELECT day, SUM(amount) FROM expenses GROUP BY day"}))
        .await
        .expect("host.table.chart");
    let chart = extract_structured(&result);

    let columns = chart["profile"]["columns"].as_array().expect("profile.columns array");
    let day_col = columns.iter().find(|c| c["name"] == "day").expect("day column present");
    assert_eq!(day_col["is_temporal"], false, "profile: {}", chart["profile"]);
    assert_eq!(chart["recommendation"]["mark"], "bar", "recommendation: {}", chart["recommendation"]);
}

#[tokio::test]
async fn iso_date_day_is_temporal_and_recommends_line() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Chart AC4b Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    chart_fixture::seed_events(&client).await;

    let result = client
        .tools_call("host.table.chart", serde_json::json!({"sql": "SELECT day, SUM(amount) FROM events GROUP BY day"}))
        .await
        .expect("host.table.chart");
    let chart = extract_structured(&result);

    let columns = chart["profile"]["columns"].as_array().expect("profile.columns array");
    let day_col = columns.iter().find(|c| c["name"] == "day").expect("day column present");
    assert_eq!(day_col["is_temporal"], true, "profile: {}", chart["profile"]);
    assert_eq!(day_col["data_type"], "temporal", "profile: {}", chart["profile"]);
    assert_eq!(chart["recommendation"]["mark"], "line", "recommendation: {}", chart["recommendation"]);
}
