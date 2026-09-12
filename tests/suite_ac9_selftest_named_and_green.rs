//! PRD-mcphost-test-suite-consolidation
//! AC9 (P1) -- Given the `suite` selftest cases, When the repo's test
//! selftest runs, Then they are named and green.
//!
//! `test_prefix: suite` (this PRD's frontmatter) means the verified-
//! completed pairing classifier looks for `tests/suite_ac<N>_*.rs` files to
//! pair each of this PRD's own P0/P1 acceptance criteria. This file is the
//! completeness check on that set itself: every applicable AC number (1-9,
//! minus AC8, deferred -- see the PRD's Receipts/frontmatter for why) must
//! have exactly one `tests/suite_ac<N>_*.rs` file, so a future edit can't
//! silently drop one of this PRD's own self-tests without the crate's own
//! test suite catching it. "Green" is a property of `cargo test`/`cargo
//! nextest run` passing at all (this file included), not re-asserted here.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// AC8 (P1, parity script matching) is deferred -- see the PRD's Open
/// Questions ("Should the burst-lane parity script match on test names now
/// that suites are few?", owned by Joe, due at ship) and `deferred_acs` in
/// its frontmatter. `scripts/burst-lane.sh` lives outside this repo in the
/// shared build-skill and is dormant (not configured) on this host; wiring
/// it is a separate, cross-repo decision this PRD does not force through.
const DEFERRED_ACS: &[u32] = &[8];
const APPLICABLE_ACS: [u32; 9] = [1, 2, 3, 4, 5, 6, 7, 8, 9];

#[test]
fn every_applicable_ac_has_exactly_one_selftest_file() {
    let tests_dir = repo_root().join("tests");
    let mut found: BTreeSet<u32> = BTreeSet::new();
    let mut dupes: Vec<String> = Vec::new();

    for entry in fs::read_dir(&tests_dir).expect("read tests/").flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(rest) = name.strip_prefix("suite_ac") else {
            continue;
        };
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            continue;
        }
        let n: u32 = digits.parse().unwrap();
        if !found.insert(n) {
            dupes.push(name);
        }
    }

    assert!(
        dupes.is_empty(),
        "more than one tests/suite_ac<N>_*.rs file claims the same AC number: {dupes:?}"
    );

    let mut missing: Vec<u32> = Vec::new();
    for &n in APPLICABLE_ACS.iter() {
        if DEFERRED_ACS.contains(&n) {
            assert!(
                !found.contains(&n),
                "AC{n} is listed as deferred in this test but a \
                 tests/suite_ac{n}_*.rs file exists -- update DEFERRED_ACS \
                 (and the PRD frontmatter) or remove the deferral"
            );
            continue;
        }
        if !found.contains(&n) {
            missing.push(n);
        }
    }
    assert!(
        missing.is_empty(),
        "no tests/suite_ac{{{}}}_*.rs selftest file found for these non-deferred ACs",
        missing.iter().map(u32::to_string).collect::<Vec<_>>().join(",")
    );
}
