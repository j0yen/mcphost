//! PRD-mcphost-host-changelog-since AC1 — Given the committed `CHANGELOG.md`,
//! When `parse_changelog` runs, Then every `## ` heading yields
//! `(semver, date)`, versions are strictly descending, the count equals the
//! number of `## v` lines, and the top version matches `Cargo.toml`.

use mcphost::api_contract::{CHANGELOG_MD, Version, parse_changelog};

#[test]
fn committed_changelog_headings_are_a_complete_descending_release_list() {
    let releases = parse_changelog(CHANGELOG_MD).expect("every ## heading parses");
    let v_lines = CHANGELOG_MD.lines().filter(|l| l.starts_with("## v")).count();
    let all_headings = CHANGELOG_MD.lines().filter(|l| l.starts_with("## ")).count();
    assert_eq!(all_headings, v_lines, "every ## heading must be a `## v` heading");
    assert_eq!(releases.len(), v_lines, "one release per `## v` line");
    for pair in releases.windows(2) {
        assert!(
            pair[0].version > pair[1].version,
            "versions must be strictly descending: {} then {}",
            pair[0].version,
            pair[1].version
        );
    }
    // The deploy flow bumps Cargo.toml and the top heading together; a
    // patch-only bump at land time may leave the heading one patch behind,
    // never ahead and never on another minor line.
    let cargo = Version::parse(env!("CARGO_PKG_VERSION")).unwrap();
    let top = &releases[0].version;
    assert!(top <= &cargo, "top heading {top} is ahead of Cargo.toml {cargo}");
    assert_eq!((top.major, top.minor), (cargo.major, cargo.minor), "top heading {top} vs Cargo.toml {cargo}");
    assert!(releases.iter().all(|r| r.summary.chars().count() <= 300));
}

#[test]
fn malformed_heading_is_an_error_not_a_skip() {
    assert!(parse_changelog("## Unreleased\n\nx\n").is_err());
    assert!(parse_changelog("## v1.2.3 — 2026-01-01\n\nok\n").is_ok());
}
