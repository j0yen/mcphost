//! PRD-mcphost-timeseries-buckets
//! AC1 -- Given `examples/uptime-probes/tools/status.py` (copied as it stood
//! before R6) and the tool sources of the latest explore run under
//! `tests/fixtures/tsb_tool_sources/`, When a test greps them for `statistics.`,
//! `sum(`, `max(`, `min(`, `len(` applied to rows from `mcphost.state.query` /
//! `mcphost.table.query`, Then at least one hit exists and the count is printed.
//! Zero hits would confirm the falsifier and stop the build.

use std::fs;
use std::path::Path;

const AGGREGATES: &[&str] = &["statistics.", "sum(", "max(", "min(", "len("];
const ROW_SOURCES: &[&str] = &["mcphost.state.query", "mcphost.table.query"];

/// Aggregate-call lines in a tool source that fetches rows from a row source.
fn aggregation_hits(src: &str) -> Vec<String> {
    if !ROW_SOURCES.iter().any(|s| src.contains(s)) {
        return Vec::new();
    }
    src.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#') && AGGREGATES.iter().any(|a| l.contains(a)))
        .map(str::to_string)
        .collect()
}

#[test]
fn tsb_ac01_tool_code_aggregates_over_fetched_rows() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tsb_tool_sources");
    let mut total = 0;
    let mut sources = 0;
    for entry in fs::read_dir(&dir).expect("fixture dir") {
        let path = entry.expect("entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("py") {
            continue;
        }
        sources += 1;
        let hits = aggregation_hits(&fs::read_to_string(&path).expect("read fixture"));
        for h in &hits {
            println!("tsb_ac01 hit {}: {h}", path.file_name().unwrap().to_string_lossy());
        }
        total += hits.len();
    }
    println!("tsb_ac01 aggregation hits: {total} across {sources} tool sources");
    assert!(sources >= 2, "fixture dir must hold status.py and an explore tool source");
    assert!(total >= 1, "0 hits: the falsifier is confirmed, stop the build and record it");
}
