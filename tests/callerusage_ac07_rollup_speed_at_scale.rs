//! PRD-mcphost-shared-tool-caller-usage
//! AC7 (P1) — Given 1M meter events over 30 days, When
//! `host.usage {window: "30d"}` runs after the daily rollup, Then it
//! answers in < 200 ms from `usage_daily`.
//!
//! The PRD's own non-functional target ("`host.usage` over 30 days answers
//! in < 200 ms at 1M events via the rollup") is about the breakdown path
//! this PRD adds (`Db::usage_breakdown`'s `usage_daily` fast path,
//! requirement 6) -- the legacy no-`by` shape stays reading raw `calls`
//! byte for byte unchanged (AC6), so this test exercises `by: "tool"`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;
use std::time::Instant;

const TOTAL_CALLS: i64 = 1_000_000;
// Strictly less than 30: `host.usage {window: "30d"}` only counts
// usage_daily days >= `now - 30d`'s OWN calendar day, so every seeded day
// must land inside that window with room to spare -- 28 fully-elapsed
// days, ending yesterday, leaves a two-day margin on both ends.
const SPREAD_DAYS: i64 = 28;

#[tokio::test]
async fn thirty_day_breakdown_answers_fast_after_rollup() {
    let server = TestServer::start().await;
    let (ns_o, key_o) = signup(&server.base_url, "Owner O").await;
    let client_o = McpClient::with_bearer(&server.base_url, &key_o);
    let owner = server
        .state
        .db
        .find_tenant_by_namespace(ns_o)
        .await
        .expect("db query")
        .expect("owner exists");

    let now = mcphost::state::now_unix();
    let today_start = now - now.rem_euclid(86_400);
    // Every seeded day is fully elapsed (< today_start, so the rollup
    // below actually covers it -- see Db::rollup_usage_daily's own doc
    // comment) and within the 30d window `since` boundary with margin to
    // spare on both ends.
    let base_unix = today_start - (SPREAD_DAYS + 1) * 86_400;
    server
        .state
        .db
        .insert_calls_bulk_spread_for_test(owner.id, "loadtest".to_string(), TOTAL_CALLS, base_unix, SPREAD_DAYS)
        .await
        .expect("seed 1M calls");

    let rolled = server
        .state
        .db
        .rollup_usage_daily(now)
        .await
        .expect("rollup_usage_daily");
    assert!(rolled > 0, "the rollup must have written at least one usage_daily row");

    let start = Instant::now();
    let usage = client_o
        .tools_call("host.usage", json!({"by": "tool", "window": "30d"}))
        .await
        .expect("host.usage by tool over 30d");
    let elapsed = start.elapsed();

    let body = extract_structured(&usage);
    let rows = body["rows"].as_array().expect("rows array");
    let row = rows
        .iter()
        .find(|r| r["key"] == json!("loadtest"))
        .expect("loadtest row must be present");
    assert_eq!(row["calls"], json!(TOTAL_CALLS), "{row}");
    assert!(
        elapsed.as_millis() < 200,
        "30d host.usage must answer in < 200ms from usage_daily, took {elapsed:?}"
    );
}
