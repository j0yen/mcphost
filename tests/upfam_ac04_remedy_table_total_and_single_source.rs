//! AC4 — Given `UPSTREAM_REMEDIES`, When a test iterates statuses 400..=599
//! and the `host_not_allowed` row, Then every status maps to exactly one row
//! and every rendered hint equals that row's text — and a scan of
//! `src/kinds/http.rs` finds no `"hint"` string literal outside the table
//! block.

use mcphost::kinds::http::{UPSTREAM_REMEDIES, remedy_for};

#[test]
fn every_status_maps_to_exactly_one_row_and_renders_its_hint() {
    for status in 400u16..=599 {
        let matching: Vec<_> = UPSTREAM_REMEDIES
            .iter()
            .filter(|r| r.statuses.iter().any(|range| range.contains(&status)))
            .collect();
        assert_eq!(matching.len(), 1, "status {status} matched {} rows", matching.len());
        let rendered = remedy_for(status);
        assert_eq!(rendered.code, matching[0].code, "status {status}");
        assert_eq!(rendered.hint, matching[0].hint, "status {status}");
    }
    let hna: Vec<_> = UPSTREAM_REMEDIES
        .iter()
        .filter(|r| r.code == "host_not_allowed")
        .collect();
    assert_eq!(hna.len(), 1, "exactly one host_not_allowed row");
    assert!(hna[0].statuses.is_empty(), "host_not_allowed answers no status");
    assert!(!hna[0].hint.is_empty());
}

#[test]
fn no_hint_literal_outside_the_table_block() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/kinds/http.rs"))
        .expect("read http.rs");
    let begin = src.find("// BEGIN UPSTREAM_REMEDIES").expect("begin marker");
    let end = src.find("// END UPSTREAM_REMEDIES").expect("end marker");
    assert!(begin < end);
    // This test's own file is not scanned; the needle is built so it never
    // appears as a quoted literal in `http.rs` through this test.
    let needle = format!("\"{}\"", "hint");
    let outside: Vec<_> = src
        .match_indices(&needle)
        .filter(|(i, _)| *i < begin || *i > end)
        .map(|(i, _)| src[..i].lines().count())
        .collect();
    assert!(
        outside.is_empty(),
        "\"hint\" literal outside the UPSTREAM_REMEDIES block at lines {outside:?}"
    );
}
