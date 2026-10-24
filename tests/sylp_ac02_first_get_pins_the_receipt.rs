//! contract: reads recipes/data-sync.toml, examples/data-sync/orders.csv
//! sylp AC2 (PRD-mcphost-sync-pull-change-detection-live-proof R1): the first
//! `host.sync.get` of the data-sync recipe expects `status` = ok,
//! `change_detected` = true and `rows_added` equal to the data-row count of
//! `examples/data-sync/orders.csv`; the test fails when the CSV gains or loses a
//! row without the recipe changing.

use mcphost::recipes::{Expect, Recipe, Step};
use serde_json::json;
use std::path::Path;

fn repo() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn recipe() -> Recipe {
    let text = std::fs::read_to_string(repo().join("recipes/data-sync.toml")).expect("read data-sync.toml");
    toml::from_str(&text).expect("parse recipe")
}

/// Non-empty lines of the CSV minus its header line.
fn csv_data_rows() -> usize {
    let csv = std::fs::read_to_string(repo().join("examples/data-sync/orders.csv")).expect("read orders.csv");
    csv.lines().filter(|l| !l.trim().is_empty()).count().saturating_sub(1)
}

fn expect_of<'a>(s: &'a Step, path: &str) -> &'a Expect {
    s.expect.iter().find(|e| e.path == path).unwrap_or_else(|| panic!("the first host.sync.get has no expect on {path}"))
}

#[test]
fn sylp_ac02_first_get_expects_ok_changed_and_the_csv_row_count() {
    let r = recipe();
    let first = r.steps.iter().find(|s| s.tool.as_deref() == Some("host.sync.get")).expect("a host.sync.get step");
    assert_eq!(expect_of(first, "$.last_run.status").value, Some(json!("ok")));
    assert_eq!(expect_of(first, "$.last_run.change_detected").value, Some(json!(true)));
    let rows = csv_data_rows();
    assert!(rows > 0, "orders.csv has no data rows");
    assert_eq!(
        expect_of(first, "$.last_run.rows_added").value,
        Some(json!(rows)),
        "rows_added in recipes/data-sync.toml must equal the {rows} data rows of examples/data-sync/orders.csv"
    );
}

#[test]
fn sylp_ac02_proof_default_row_count_matches_the_csv() {
    let r = recipe();
    let want = format!("DATA_SYNC_ROWS=\"${{DATA_SYNC_ROWS:-{}}}\"", csv_data_rows());
    assert!(r.proof.body.contains(&want), "proof body must default DATA_SYNC_ROWS to the CSV's data rows: {want}");
}

#[test]
fn sylp_ac02_the_stale_not_pinned_comment_is_gone() {
    let text = std::fs::read_to_string(repo().join("recipes/data-sync.toml")).expect("read data-sync.toml");
    assert!(!text.contains("deliberately not pinned"), "the recipe still claims status is not pinned");
    assert!(!text.contains("not_implemented"), "the recipe still mentions the not_implemented status");
}
