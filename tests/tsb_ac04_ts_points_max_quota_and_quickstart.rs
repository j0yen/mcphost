//! PRD-mcphost-timeseries-buckets
//! AC4 -- Given a plan with `ts_points_max = 10`, When the 11th put arrives,
//! Then `quota_exceeded` names `ts_points_max` with `used: 10`, and
//! `host.quickstart.limits.plan.ts_points_max` reads 10 with no code in
//! `quickstart_limits` naming it.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

const CEILING: i64 = 10;

/// The source text of `fn quickstart_limits` in `src/control.rs`, up to the next top-level item.
fn quickstart_limits_source() -> String {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/control.rs")).expect("src/control.rs");
    let start = src.find("fn quickstart_limits(").expect("quickstart_limits exists");
    let rest = &src[start..];
    let end = rest.find("\n}\n").expect("end of fn");
    rest[..end].to_string()
}

#[tokio::test]
async fn tsb_ac04_the_eleventh_put_is_quota_exceeded_and_quickstart_reads_the_plan_field() {
    let server = TestServer::start_with_plans(|plans| {
        for plan in plans.plans.iter_mut() {
            plan.ts_points_max = CEILING;
        }
    })
    .await;
    let (_ns, key) = signup(&server.base_url, "tsb ac4").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    for i in 0..CEILING {
        let put = client
            .tools_call("host.ts.put", json!({"series": "latency_ms", "value": i, "ts": 1_700_000_000 + i}))
            .await
            .unwrap_or_else(|e| panic!("put {i} is within the ceiling: {} {}", e.code, e.message));
        assert_eq!(extract_structured(&put)["points"], json!(i + 1));
    }
    let err = client
        .tools_call("host.ts.put", json!({"series": "latency_ms", "value": 99, "ts": 1_700_000_100}))
        .await
        .expect_err("the 11th put is refused");
    assert_eq!(err.error_code.as_deref(), Some("quota_exceeded"), "{} {}", err.message, err.data);
    assert_eq!(err.data["limit"]["name"], json!("ts_points_max"), "{}", err.data);
    assert_eq!(err.data["limit"]["value"], json!(CEILING), "{}", err.data);
    assert_eq!(err.data["used"], json!(CEILING), "{}", err.data);
    // The envelope owns `data.remedy` (the registry's sentence for the code), so the
    // verb's remedy rides in `data.next.remedy` and the message.
    assert_eq!(err.data["next"]["remedy"], json!("host.ts.trim or billing.checkout"), "{}", err.data);
    assert!(err.message.contains("ts_points_max") && err.message.contains("host.ts.trim or billing.checkout"), "{}", err.message);

    // The refused put wrote nothing; another series has its own room.
    let got = client
        .tools_call("host.ts.get", json!({"series": "latency_ms", "bucket": "1d", "from": 1_699_999_000, "to": 1_700_001_000}))
        .await
        .expect("get");
    let got = extract_structured(&got);
    let total: i64 = got["buckets"].as_array().expect("buckets").iter().map(|b| b["count"].as_i64().unwrap()).sum();
    assert_eq!(total, CEILING, "{got}");
    client.tools_call("host.ts.put", json!({"series": "other_series", "value": 1})).await.expect("another series is under its own ceiling");

    // The quickstart number is the Plan field, rendered by `ceilings_json`.
    let quickstart = client.tools_call("host.quickstart", json!({"kind": "echo"})).await.expect("host.quickstart");
    let quickstart = extract_structured(&quickstart);
    assert_eq!(quickstart["limits"]["plan"]["ts_points_max"], json!(CEILING), "{quickstart}");
    assert!(!quickstart_limits_source().contains("ts_points_max"), "quickstart_limits must not name the field");
}
