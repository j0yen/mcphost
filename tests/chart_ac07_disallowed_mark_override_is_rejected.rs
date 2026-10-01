//! PRD-mcphost-chart-in-a-minute
//! AC7 — Given `mark: "pie"` when the alternatives do not include it, When
//! chart runs, Then a validation error names the allowed marks and no
//! chart is stored.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};

use crate::chart_fixture;

#[tokio::test]
async fn pie_mark_override_is_rejected_naming_allowed_marks_and_stores_nothing() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Chart AC7 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    chart_fixture::seed_expenses(&client).await;

    let err = client
        .tools_call(
            "host.table.chart",
            serde_json::json!({
                "sql": "SELECT category, SUM(amount) AS total FROM expenses GROUP BY category",
                "mark": "pie",
                "share": true,
            }),
        )
        .await
        .expect_err("mark: pie must be rejected");
    assert_eq!(err.error_code.as_deref(), Some("chart_invalid_mark"), "err: {err:?}");

    let allowed = err.data["allowed"].as_array().expect("data.allowed array");
    let allowed_strs: Vec<&str> = allowed.iter().filter_map(|v| v.as_str()).collect();
    assert!(allowed_strs.contains(&"bar"), "allowed marks must include bar: {allowed_strs:?}");
    assert!(!allowed_strs.contains(&"pie"), "allowed marks must never include pie: {allowed_strs:?}");

    // No chart was stored -- host.table.charts lists none.
    let listing = client.tools_call("host.table.charts", serde_json::json!({})).await.expect("host.table.charts");
    let charts = extract_structured(&listing);
    assert_eq!(
        charts["charts"].as_array().expect("charts array").len(),
        0,
        "a rejected mark override must store nothing: {charts}"
    );
}
