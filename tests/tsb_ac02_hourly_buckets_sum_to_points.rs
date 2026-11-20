//! PRD-mcphost-timeseries-buckets
//! AC2 -- Given 1,000 `host.ts.put` points over 24 h in series `latency_ms`,
//! When `host.ts.get(bucket="1h", from="-24h", to="now")` runs, Then 24 buckets
//! return with `count` summing to 1,000 and each bucket's `p95` >= `p50` >= `min`.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

const POINTS: i64 = 1_000;
const HOUR_S: i64 = 3_600;

fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("clock").as_secs() as i64
}

#[tokio::test]
async fn tsb_ac02_thousand_points_make_24_hourly_buckets() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "tsb ac2").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    // 1,000 points spread over the 24 h that end one minute ago; ts moves 86.34 s
    // per point, so every hour of the window receives points (41-42 each).
    let end = now_unix() - 60;
    let span = 24 * HOUR_S - 120;
    for i in 0..POINTS {
        let ts = end - span + i * span / (POINTS - 1);
        let value = 50.0 + ((i * 37) % 200) as f64;
        let put = client
            .tools_call("host.ts.put", json!({"series": "latency_ms", "value": value, "ts": ts}))
            .await
            .unwrap_or_else(|e| panic!("put {i}: {} {}", e.code, e.message));
        let put = extract_structured(&put);
        assert_eq!(put["series"], json!("latency_ms"), "{put}");
        assert_eq!(put["points"], json!(i + 1), "{put}");
    }

    let got = client
        .tools_call("host.ts.get", json!({"series": "latency_ms", "bucket": "1h", "from": "-24h", "to": "now"}))
        .await
        .unwrap_or_else(|e| panic!("get: {} {}", e.code, e.message));
    let got = extract_structured(&got);
    assert_eq!(got["bucket_s"], json!(HOUR_S), "{got}");
    let buckets = got["buckets"].as_array().expect("buckets array");
    assert_eq!(buckets.len(), 24, "24 hourly buckets: {got}");
    let total: i64 = buckets.iter().map(|b| b["count"].as_i64().expect("count")).sum();
    assert_eq!(total, POINTS, "counts sum to every point: {got}");
    for b in buckets {
        let (min, max, avg) = (b["min"].as_f64().unwrap(), b["max"].as_f64().unwrap(), b["avg"].as_f64().unwrap());
        let (p50, p95) = (b["p50"].as_f64().unwrap(), b["p95"].as_f64().unwrap());
        assert!(p95 >= p50 && p50 >= min, "p95 >= p50 >= min in {b}");
        assert!(max >= p95 && avg >= min && avg <= max, "max >= p95 and min <= avg <= max in {b}");
    }
    let ts: Vec<i64> = buckets.iter().map(|b| b["ts"].as_i64().unwrap()).collect();
    assert!(ts.windows(2).all(|w| w[1] - w[0] == HOUR_S), "buckets are consecutive hours: {ts:?}");
}
