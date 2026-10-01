//! PRD-mcphost-chart-in-a-minute: a deterministic 1,000-row `expenses`
//! fixture (reused across `chart_ac02`..`chart_ac14`, same "one shared
//! fixture generator, not five copy-pasted CSVs" convention
//! `examples/database-in-a-minute/fixture.csv` established for the
//! database recipe) plus a small ISO-date `events` table for AC4's second
//! case. Hoisted into whichever suite `scripts/gen-test-suites.sh` buckets
//! the `chart_ac*` files into, same as every other `tests/support/*.rs`
//! file.

#![allow(dead_code)] // same reason as tests/common/mod.rs: any one AC's test file uses only a subset of this fixture

use crate::common::McpClient;
use serde_json::{Value, json};

/// Five categories, round-robin over 1,000 rows (200 rows each) -- low
/// enough cardinality that `category` reads as a `category`-role column
/// over the full table (`tables_model.rs`'s own `CATEGORY_MAX_DISTINCT`/
/// `_SHARE` thresholds), never a `key`.
pub const CATEGORIES: [&str; 5] = ["produce", "bakery", "dairy", "meat", "pantry"];

/// The 1,000-row `expenses` fixture: `id` (0..1000), `category` (round-robin
/// over [`CATEGORIES`]), `amount` (a deterministic real whose per-category
/// base (`10 + 5*category_index`) keeps category sums well-separated --
/// `pantry`, the last category, is always the largest), `day` (1..=20,
/// round-robin -- 20 distinct values over 1,000 rows, the same low-
/// cardinality shape `category` has).
pub fn expenses_rows() -> Vec<Value> {
    (0..1000i64)
        .map(|i| {
            let cat_idx = (i as usize) % CATEGORIES.len();
            let category = CATEGORIES[cat_idx];
            let day = (i % 20) + 1;
            let amount = 10.0 + (cat_idx as f64) * 5.0 + (i % 7) as f64;
            json!({"id": i, "category": category, "amount": amount, "day": day})
        })
        .collect()
}

/// Sums `amount` in [`expenses_rows`] grouped by `category`, in
/// [`CATEGORIES`] order -- the test's own independent recomputation of
/// what `host.table.chart`'s caption facts must equal.
pub fn expected_category_totals() -> Vec<(&'static str, f64)> {
    CATEGORIES
        .iter()
        .map(|&cat| {
            let total: f64 = expenses_rows()
                .into_iter()
                .filter(|r| r["category"] == cat)
                .map(|r| r["amount"].as_f64().unwrap())
                .sum();
            (cat, total)
        })
        .collect()
}

/// Sums `amount` in [`expenses_rows`] grouped by `day` (1..=20).
pub fn expected_day_totals() -> Vec<(i64, f64)> {
    (1..=20i64)
        .map(|day| {
            let total: f64 = expenses_rows()
                .into_iter()
                .filter(|r| r["day"] == json!(day))
                .map(|r| r["amount"].as_f64().unwrap())
                .sum();
            (day, total)
        })
        .collect()
}

/// requirement 1/AC2: `host.table.create` + batched `host.table.append` of
/// the full 1,000-row fixture into a table named `expenses`.
pub async fn seed_expenses(client: &McpClient) {
    client
        .tools_call(
            "host.table.create",
            json!({
                "name": "expenses",
                "columns": {"id": "integer", "category": "text", "amount": "real", "day": "integer"},
                "primary_key": "id",
            }),
        )
        .await
        .expect("create expenses");

    let rows = expenses_rows();
    for chunk in rows.chunks(200) {
        client
            .tools_call("host.table.append", json!({"table": "expenses", "rows": chunk}))
            .await
            .expect("append expenses chunk");
    }
}

/// AC4's second fixture: a table with an ISO-date `day` column (20 distinct
/// dates, 5 rows each -- low enough per-date share that `day` reads as
/// `date`-role, not `category`, over the full table) so the recommender
/// sees a temporal dimension.
pub fn events_rows() -> Vec<Value> {
    (0..100i64)
        .map(|i| {
            let date_idx = (i % 20) + 1;
            let day = format!("2026-01-{date_idx:02}");
            let amount = 10.0 + (i % 9) as f64;
            json!({"day": day, "amount": amount})
        })
        .collect()
}

pub async fn seed_events(client: &McpClient) {
    client
        .tools_call(
            "host.table.create",
            json!({"name": "events", "columns": {"day": "timestamp", "amount": "real"}}),
        )
        .await
        .expect("create events");
    client
        .tools_call("host.table.append", json!({"table": "events", "rows": events_rows()}))
        .await
        .expect("append events");
}
