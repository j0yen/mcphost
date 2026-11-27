//! PRD-mcphost-timeseries-buckets
//! AC5 -- Given 50,000 points and `bucket="1h"` over 24 h on the gate box, When
//! `host.ts.get` runs, Then it completes in <= 200 ms; Given an empty series,
//! Then `buckets: []` and no error.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use rusqlite::Connection;
use serde_json::json;

const POINTS: i64 = 50_000;
const DAY_S: i64 = 86_400;

fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("clock").as_secs() as i64
}

#[tokio::test]
async fn tsb_ac05_an_empty_series_answers_an_empty_bucket_list() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "tsb ac5 empty").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    // A series nothing was ever put into, before the tenant's store holds any ts table.
    let got = client
        .tools_call("host.ts.get", json!({"series": "never_written", "bucket": "1h", "from": "-24h", "to": "now"}))
        .await
        .unwrap_or_else(|e| panic!("empty get errored: {} {}", e.code, e.message));
    let got = extract_structured(&got);
    assert_eq!(got["buckets"], json!([]), "{got}");

    // A series with points elsewhere leaves this one empty too.
    client.tools_call("host.ts.put", json!({"series": "written", "value": 1})).await.expect("put");
    let got = client
        .tools_call("host.ts.get", json!({"series": "never_written", "bucket": "1h"}))
        .await
        .unwrap_or_else(|e| panic!("empty get errored: {} {}", e.code, e.message));
    assert_eq!(extract_structured(&got)["buckets"], json!([]));
}

#[tokio::test]
async fn tsb_ac05_one_hour_buckets_over_fifty_thousand_points_take_at_most_200_ms() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "tsb ac5 scale").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("db").expect("tenant");

    // One real put creates the table and its index; the other 49,999 points go in
    // through the tenant's own SQLite file in one transaction.
    let start = now_unix() - DAY_S + 60;
    client
        .tools_call("host.ts.put", json!({"series": "latency_ms", "value": 1, "ts": start}))
        .await
        .expect("first put");
    let db_path = server.data_dir.0.join("tables").join(format!("{}.db", tenant.id));
    {
        let mut conn = Connection::open(&db_path).expect("tenant tables file");
        let tx = conn.transaction().expect("tx");
        {
            let mut stmt = tx.prepare("INSERT INTO ts_points (series, ts, value, tags_json) VALUES ('latency_ms', ?1, ?2, NULL)").expect("prepare");
            for i in 1..POINTS {
                let ts = start + i * (DAY_S - 120) / POINTS;
                stmt.execute(rusqlite::params![ts, ((i * 7919) % 500) as f64]).expect("insert");
            }
        }
        tx.commit().expect("commit");
    }

    let args = json!({"series": "latency_ms", "bucket": "1h", "from": "-24h", "to": "now"});
    let got = client.tools_call("host.ts.get", args.clone()).await.expect("get");
    let got = extract_structured(&got);
    let buckets = got["buckets"].as_array().expect("buckets");
    assert_eq!(buckets.len(), 24, "{got}");
    let total: i64 = buckets.iter().map(|b| b["count"].as_i64().unwrap()).sum();
    assert_eq!(total, POINTS, "every point lands in a bucket");

    crate::perf_budget!(200, {
        client.tools_call("host.ts.get", args.clone()).await.expect("timed get");
    });
}
