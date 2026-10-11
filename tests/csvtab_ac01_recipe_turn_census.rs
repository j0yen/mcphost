//! PRD-mcphost-csv-to-table-one-call
//! AC1 -- Given the last three truth-tier run directories, When a test sums
//! turns between the recipe persona's first `host.table.*` call and its first
//! `host.table.query`, Then the number is printed; if the median is <= 1 turn
//! the builder records the falsifier as confirmed and ships only R5/R6.
//!
//! The census reads `synthorg consume --tier truth` run directories
//! (`runs/truth-tier-*/ledger.jsonl`, one JSON row per scored session, each
//! naming its `transcript_path`; a transcript is one JSON line per tool call
//! with `turn` and `tool`). Those directories live in `~/repos/synthorg` on
//! the operator hosts, not in this repository, so the directory root is
//! `MCPHOST_TRUTH_RUNS` (default `~/repos/synthorg/runs`). When the build
//! host has no truth-tier directory the test prints that fact, and
//! `no_directories_means_no_number_not_a_verdict` pins that an absent census
//! never produces a falsifier verdict. The reader itself is exercised on an
//! inline transcript (mechanism only -- it invents no measurement).

use serde_json::Value;
use std::path::{Path, PathBuf};

/// Task ids / personas of the database-in-a-minute recipe (the consumer task
/// `spreadsheet-owner-database-in-a-minute`, and the data-pipeline panel).
fn is_recipe_row(row: &Value) -> bool {
    let task = row["task_id"].as_str().unwrap_or("");
    let persona = row["persona_id"].as_str().unwrap_or("");
    task.contains("database-in-a-minute")
        || persona.starts_with("panel_data_pipeline_builder")
        || persona.starts_with("panel_spreadsheet_owner")
}

/// Turns from the first `host_table_*` call to the first `host_table_query`
/// call in one transcript; `None` if either never happens.
fn turns_to_first_query(transcript: &str) -> Option<i64> {
    let mut first_table: Option<i64> = None;
    for line in transcript.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let (Some(tool), Some(turn)) = (v["tool"].as_str(), v["turn"].as_i64()) else { continue };
        let tool = tool.replace('.', "_");
        if !tool.starts_with("host_table_") {
            continue;
        }
        let first = *first_table.get_or_insert(turn);
        if tool == "host_table_query" {
            return Some(turn - first);
        }
    }
    None
}

fn median(mut v: Vec<i64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_unstable();
    let n = v.len();
    Some(if n % 2 == 1 { v[n / 2] as f64 } else { (v[n / 2 - 1] + v[n / 2]) as f64 / 2.0 })
}

/// `Some(true)` = falsifier confirmed (ship only R5/R6); `None` = no data.
fn falsifier_confirmed(turns: &[i64]) -> Option<bool> {
    median(turns.to_vec()).map(|m| m <= 1.0)
}

fn runs_root() -> PathBuf {
    std::env::var_os("MCPHOST_TRUTH_RUNS")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("repos/synthorg/runs"))
}

fn last_three_truth_dirs(root: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.is_dir() && p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("truth-tier-")))
                .collect()
        })
        .unwrap_or_default();
    dirs.sort();
    let skip = dirs.len().saturating_sub(3);
    dirs.split_off(skip)
}

/// Per-session turn counts over the last three truth-tier directories.
fn census(root: &Path) -> (Vec<PathBuf>, Vec<i64>) {
    let dirs = last_three_truth_dirs(root);
    let synthorg_root = root.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut turns = Vec::new();
    for dir in &dirs {
        let Ok(ledger) = std::fs::read_to_string(dir.join("ledger.jsonl")) else { continue };
        for row in ledger.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter(is_recipe_row) {
            let Some(tp) = row["transcript_path"].as_str() else { continue };
            let candidates = [synthorg_root.join(tp), dir.join(tp), root.join(tp)];
            let Some(text) = candidates.iter().find_map(|p| std::fs::read_to_string(p).ok()) else { continue };
            if let Some(n) = turns_to_first_query(&text) {
                turns.push(n);
            }
        }
    }
    (dirs, turns)
}

#[test]
fn recipe_persona_turns_between_first_table_call_and_first_query_are_printed() {
    let root = runs_root();
    let (dirs, turns) = census(&root);
    println!("csvtab AC1: truth-tier directories read: {dirs:?}");
    match median(turns.clone()) {
        Some(m) => {
            println!("csvtab AC1: turns first host.table.* -> first host.table.query per recipe session: {turns:?}");
            println!("csvtab AC1: median = {m}");
            println!(
                "csvtab AC1: falsifier {} (median <= 1 turn confirms it and ships only R5/R6)",
                if falsifier_confirmed(&turns) == Some(true) { "CONFIRMED" } else { "not confirmed" }
            );
        }
        None => println!(
            "csvtab AC1: no recipe session with both a host.table.* call and a host.table.query under {} \
             ({} truth-tier dirs) -- no number on this host; the census runs where the runs live",
            root.display(),
            dirs.len()
        ),
    }
}

#[test]
fn reader_counts_turns_between_first_table_call_and_first_query() {
    // Mechanism only: shape of a synthorg transcript, not a measurement.
    let t = [
        r#"{"turn": 1, "tool": "signup"}"#,
        r#"{"turn": 4, "tool": "host_table_create"}"#,
        r#"{"turn": 5, "tool": "host_table_append"}"#,
        r#"{"turn": 9, "tool": "host_table_query"}"#,
        r#"{"turn": 10, "tool": "host_table_query"}"#,
    ]
    .join("\n");
    assert_eq!(turns_to_first_query(&t), Some(5));
    assert_eq!(turns_to_first_query(r#"{"turn": 1, "tool": "signup"}"#), None);
    assert_eq!(median(vec![5, 1, 9]), Some(5.0));
    assert_eq!(median(vec![1, 3]), Some(2.0));
}

#[test]
fn no_directories_means_no_number_not_a_verdict() {
    let empty = std::env::temp_dir().join(format!("csvtab-ac01-empty-{}", std::process::id()));
    std::fs::create_dir_all(&empty).expect("scratch");
    let (dirs, turns) = census(&empty);
    assert!(dirs.is_empty() && turns.is_empty());
    assert_eq!(falsifier_confirmed(&turns), None, "no data must not confirm the falsifier");
    assert_eq!(falsifier_confirmed(&[1, 0, 1]), Some(true));
    assert_eq!(falsifier_confirmed(&[4, 6, 2]), Some(false));
    std::fs::remove_dir_all(&empty).ok();
}
