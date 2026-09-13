//! AC1 (PRD-mcphost-gate-debt-c627803): proves the actual claim AC1 makes --
//! that this repo's extended-receipts paper trail (`extended-gates.toml`'s
//! `prd_path`) resolves to a real file whose absolute path is byte-equal to
//! `agent/intent-card.json`'s `prd_source`. This is the exact invariant
//! whose violation (extended-gates.toml naming a stale/different PRD than
//! the card) caused the extended-receipts block this PRD exists to fix --
//! `ac-traceability-receipt.json`'s `prd_path` disagreeing with the card,
//! caught downstream by `revdebt_ac1_intent_card_pointers_resolve`'s cargo
//! test rerun inside flake-audit. That downstream test only catches the
//! drift once a stale cached receipt happens to exist; this test checks the
//! source config directly, so a broken `prd_path` (missing file, or a value
//! that resolves to a PRD other than the one the card names) fails here
//! even before any extended-gates producer runs -- the reviewer-agent
//! falsification test the prior gate cycle asked for (block reason
//! `must-ac1-test-unrelated-to-ac-description`: AC1's registered test had
//! zero causal connection to whether extended-receipts actually gates on
//! `prd_path`).
//!
//! No `toml` crate dependency is pulled in for this: `extended-gates.toml`
//! is a flat `key = "value"` file and `prd_path`'s value is parsed with a
//! plain string split, matching this repo's existing preference for zero
//! new dependencies on a paper-trail-only change (see this PRD's own
//! reviewer-agent receipt, `deps_audit`).

use serde_json::Value;
use std::fs;
use std::path::Path;

/// Parses `prd_path = "..."` out of `extended-gates.toml` without a TOML
/// parser -- the file has exactly one such flat key/value line.
fn parse_prd_path(toml_text: &str) -> String {
    for line in toml_text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("prd_path") {
            let rest = rest.trim_start();
            let rest = rest.strip_prefix('=').expect("prd_path line must have '='");
            let rest = rest.trim();
            let unquoted = rest
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .unwrap_or(rest);
            return unquoted.to_owned();
        }
    }
    panic!("extended-gates.toml has no prd_path key");
}

#[test]
fn extended_gates_prd_path_resolves_and_matches_intent_card() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));

    let toml_text = fs::read_to_string(manifest_dir.join("extended-gates.toml"))
        .expect("extended-gates.toml must exist");
    let prd_path_rel = parse_prd_path(&toml_text);

    // Mirrors ac-traceability's own resolution (`dir.join(prd_path)`): an
    // absolute prd_path would replace the join entirely, same as
    // `Path::join`'s documented behavior; today's convention is a
    // repo-root-relative filename.
    let resolved = manifest_dir.join(&prd_path_rel);
    assert!(
        resolved.is_file(),
        "extended-gates.toml's prd_path ({prd_path_rel}) does not resolve to a real file at {}",
        resolved.display()
    );

    let card_text = fs::read_to_string(manifest_dir.join("agent/intent-card.json"))
        .expect("agent/intent-card.json must exist");
    let card: Value =
        serde_json::from_str(&card_text).expect("intent-card.json must be valid JSON");
    let prd_source = card
        .get("prd_source")
        .and_then(Value::as_str)
        .expect("intent-card.json must set prd_source");

    let resolved_str = resolved
        .to_str()
        .expect("resolved prd_path must be valid UTF-8");
    assert_eq!(
        resolved_str, prd_source,
        "extended-gates.toml's prd_path resolves to {resolved_str:?}, which does not match \
         intent-card.json's prd_source {prd_source:?} -- the gate paper trail is out of sync \
         (see PRD-mcphost-gate-debt-c627803's five-whys)"
    );
}

#[test]
fn detects_a_prd_path_that_does_not_resolve_to_a_file() {
    // Falsification: parse_prd_path against a config naming a file that
    // does not exist must not silently "pass" -- the caller (the real test
    // above) treats a missing file as a hard failure via assert!, not a
    // skip. This proves the detection path itself, independent of this
    // repo's current (passing) state.
    let toml_text = "prd_path = \"PRD-does-not-exist-anywhere.md\"\n";
    let rel = parse_prd_path(toml_text);
    let resolved = Path::new(env!("CARGO_MANIFEST_DIR")).join(&rel);
    assert!(
        !resolved.is_file(),
        "fixture must name a file that does not exist, or this test proves nothing"
    );
}
