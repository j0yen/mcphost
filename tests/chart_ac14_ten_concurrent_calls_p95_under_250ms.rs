//! PRD-mcphost-chart-in-a-minute
//! AC14 — Given ten concurrent chart calls for one tenant, When they
//! finish, Then all ten return `chart.v1` and p95 latency on the builder
//! is under 250 ms.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

use crate::chart_fixture;

#[tokio::test]
async fn ten_concurrent_chart_calls_all_succeed_under_p95_250ms() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Chart AC14 Tenant").await;
    let seeding_client = McpClient::with_bearer(&server.base_url, &key);
    chart_fixture::seed_expenses(&seeding_client).await;

    const N: usize = 10;
    // One round = N concurrent chart calls, all of which must return chart.v1.
    // The wall time of the whole round bounds every call's latency (>= p95).
    // Median-of-5 warm rounds; skipped under MCPHOST_PERF_SKIP=1 (loaded host).
    crate::perf_budget!(250, {
        let mut tasks = Vec::with_capacity(N);
        for _ in 0..N {
            let base_url = server.base_url.clone();
            let key = key.clone();
            tasks.push(tokio::spawn(async move {
                let client = McpClient::with_bearer(&base_url, &key);
                let result = client
                    .tools_call(
                        "host.table.chart",
                        json!({"sql": "SELECT category, SUM(amount) AS total FROM expenses GROUP BY category"}),
                    )
                    .await
                    .expect("host.table.chart");
                extract_structured(&result)
            }));
        }
        for task in tasks {
            let chart = task.await.expect("task must not panic");
            assert_eq!(chart["schema"], json!("chart.v1"), "chart: {chart}");
        }
    });
}
