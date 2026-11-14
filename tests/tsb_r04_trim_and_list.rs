//! PRD-mcphost-timeseries-buckets
//! R4 -- `host.ts.trim {series, keep_last? | before_ts?}` deletes points and
//! `host.ts.list` returns every series with `points`, `oldest_ts`, `newest_ts`.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

async fn put(client: &McpClient, series: &str, ts: i64) {
    client
        .tools_call("host.ts.put", json!({"series": series, "value": ts as f64, "ts": ts}))
        .await
        .unwrap_or_else(|e| panic!("put {series}@{ts}: {} {}", e.code, e.message));
}

async fn call(client: &McpClient, tool: &str, args: Value) -> Value {
    extract_structured(&client.tools_call(tool, args).await.unwrap_or_else(|e| panic!("{tool}: {} {}", e.code, e.message)))
}

#[tokio::test]
async fn tsb_r04_list_names_every_series_and_trim_cuts_by_count_or_time() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "tsb r4").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let empty = call(&client, "host.ts.list", json!({})).await;
    assert_eq!((empty["series"].clone(), empty["truncated"].clone()), (json!([]), json!(false)), "{empty}");
    for ts in [100, 200, 300, 400, 500] {
        put(&client, "up", ts).await;
    }
    for ts in [10, 20, 30] {
        put(&client, "latency_ms", ts).await;
    }
    let list = call(&client, "host.ts.list", json!({})).await;
    assert_eq!(
        list["series"],
        json!([
            {"series": "latency_ms", "points": 3, "oldest_ts": 10, "newest_ts": 30},
            {"series": "up", "points": 5, "oldest_ts": 100, "newest_ts": 500},
        ]),
        "{list}"
    );

    let kept = call(&client, "host.ts.trim", json!({"series": "up", "keep_last": 3})).await;
    assert_eq!((kept["deleted"].clone(), kept["points"].clone(), kept["oldest_ts"].clone()), (json!(2), json!(3), json!(300)), "{kept}");
    let cut = call(&client, "host.ts.trim", json!({"series": "up", "before_ts": 400})).await;
    assert_eq!((cut["deleted"].clone(), cut["points"].clone(), cut["oldest_ts"].clone()), (json!(1), json!(2), json!(400)), "{cut}");
    // The other series is untouched.
    let list = call(&client, "host.ts.list", json!({})).await;
    assert_eq!(list["series"][0], json!({"series": "latency_ms", "points": 3, "oldest_ts": 10, "newest_ts": 30}), "{list}");
    // A trim that empties a series removes it from the list.
    let all = call(&client, "host.ts.trim", json!({"series": "up", "keep_last": 0})).await;
    assert_eq!((all["deleted"].clone(), all["points"].clone(), all["oldest_ts"].clone()), (json!(2), json!(0), Value::Null), "{all}");
    assert_eq!(call(&client, "host.ts.list", json!({})).await["series"].as_array().map(Vec::len), Some(1));

    // Exactly one cut.
    for args in [json!({"series": "up"}), json!({"series": "up", "keep_last": 1, "before_ts": 5})] {
        assert!(client.tools_call("host.ts.trim", args.clone()).await.is_err(), "{args} is refused");
    }
}

#[tokio::test]
async fn tsb_r04_a_trim_frees_room_under_ts_points_max() {
    let server = TestServer::start_with_plans(|plans| {
        for plan in plans.plans.iter_mut() {
            plan.ts_points_max = 3;
        }
    })
    .await;
    let (_ns, key) = signup(&server.base_url, "tsb r4 quota").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    for ts in [1, 2, 3] {
        put(&client, "up", ts).await;
    }
    assert!(client.tools_call("host.ts.put", json!({"series": "up", "value": 1, "ts": 4})).await.is_err(), "full");
    call(&client, "host.ts.trim", json!({"series": "up", "keep_last": 1})).await;
    put(&client, "up", 4).await;
}
