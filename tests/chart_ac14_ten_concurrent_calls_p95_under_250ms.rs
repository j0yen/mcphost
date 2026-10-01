//! PRD-mcphost-chart-in-a-minute
//! AC14 — Given ten concurrent chart calls for one tenant, When they
//! finish, Then all ten return `chart.v1` and p95 latency on the builder
//! is under 250 ms.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;
use std::time::{Duration, Instant};

use crate::chart_fixture;

#[tokio::test]
async fn ten_concurrent_chart_calls_all_succeed_under_p95_250ms() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Chart AC14 Tenant").await;
    let seeding_client = McpClient::with_bearer(&server.base_url, &key);
    chart_fixture::seed_expenses(&seeding_client).await;

    const N: usize = 10;
    let mut tasks = Vec::with_capacity(N);
    for _ in 0..N {
        let base_url = server.base_url.clone();
        let key = key.clone();
        tasks.push(tokio::spawn(async move {
            let client = McpClient::with_bearer(&base_url, &key);
            let start = Instant::now();
            let result = client
                .tools_call(
                    "host.table.chart",
                    json!({"sql": "SELECT category, SUM(amount) AS total FROM expenses GROUP BY category"}),
                )
                .await
                .expect("host.table.chart");
            let elapsed = start.elapsed();
            let chart = extract_structured(&result);
            (elapsed, chart)
        }));
    }

    let mut durations = Vec::with_capacity(N);
    for task in tasks {
        let (elapsed, chart) = task.await.expect("task must not panic");
        assert_eq!(chart["schema"], json!("chart.v1"), "chart: {chart}");
        durations.push(elapsed);
    }

    durations.sort();
    let p95 = durations[(durations.len() as f64 * 0.95).ceil() as usize - 1];
    assert!(
        p95 < Duration::from_millis(250),
        "p95 latency over {N} concurrent chart calls was {p95:?}, expected < 250ms"
    );
}
