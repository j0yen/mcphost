//! PRD-mcphost-host-changelog-since-b AC2 — Given a `CHANGELOG.md` fixture
//! with a heading out of order, When the parser runs, Then it returns
//! `ChangelogError::NotMonotonic {at}` and the tool responds with
//! `releases: []`, `releases_error` set, other fields intact.

use mcphost::api_contract::{ChangelogError, changelog_from, parse_changelog};
use mcphost::kinds::KindRegistry;

const OUT_OF_ORDER: &str = "# Changelog\n\n\
## v0.108.0 — 2026-10-14\n\neight\n\n\
## v0.106.0 — 2026-10-10\n\nsix\n\n\
## v0.107.0 — 2026-10-12\n\nseven\n";

#[test]
fn chgb_ac02_out_of_order_heading_is_not_monotonic_and_tool_degrades() {
    let parsed = parse_changelog(OUT_OF_ORDER);
    assert_eq!(parsed, Err(ChangelogError::NotMonotonic { at: 11 }));

    let out = changelog_from(&KindRegistry::with_builtin(), &[], &parsed, "0.108.0", Some("0.0.0"));
    assert_eq!(out["releases"].as_array().expect("releases array").len(), 0, "{out}");
    assert!(out["releases_error"].as_str().is_some_and(|e| e.contains("line 11")), "{out}");
    assert_eq!(out["since"], "0.0.0");
    assert_eq!(out["now"], "0.108.0");
    assert!(!out["added"].as_array().unwrap().is_empty(), "other fields intact: {out}");
    assert!(!out["renamed"].as_array().unwrap().is_empty(), "other fields intact: {out}");
    assert!(out["deprecated"].is_array() && out["removed"].is_array());
}

#[test]
fn chgb_ac02_equal_versions_are_not_strictly_descending() {
    assert_eq!(
        parse_changelog("## v1.0.0 — 2026-01-01\n\na\n\n## v1.0.0 — 2026-01-01\n\nb\n"),
        Err(ChangelogError::NotMonotonic { at: 5 })
    );
}
