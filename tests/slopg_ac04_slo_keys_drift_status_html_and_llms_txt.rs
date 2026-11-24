//! PRD-mcphost-slo-targets-on-status AC4 (P0) — Given `SLO_TARGETS`, When
//! the drift test runs, Then every `key` appears in `www/status.html` and
//! `www/llms.txt` and no other `slo.` key appears in either file. A generated
//! contract, not prose: the key set is read from the const, never spelled here.

use mcphost::statusfeed::SLO_TARGETS;
use std::collections::BTreeSet;
use std::path::Path;

fn read(rel: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)).expect("read www file")
}

/// Every `slo.<ident>` a file spells, other than as the tail of a longer
/// path (`.slo.` with a word character before `slo` is not a key reference).
fn slo_keys_in(text: &str) -> BTreeSet<String> {
    let bytes = text.as_bytes();
    let mut keys = BTreeSet::new();
    for (at, _) in text.match_indices("slo.") {
        if at > 0 && (bytes[at - 1].is_ascii_alphanumeric() || bytes[at - 1] == b'_') {
            continue;
        }
        let key: String = text[at + 4..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if !key.is_empty() {
            keys.insert(key);
        }
    }
    keys
}

#[test]
fn slopg_ac04_every_slo_key_is_in_both_files_and_no_other_slo_key_is() {
    let want: BTreeSet<String> = SLO_TARGETS.iter().map(|t| t.key.to_string()).collect();
    assert_eq!(want.len(), SLO_TARGETS.len(), "SLO_TARGETS keys are unique");
    for file in ["www/status.html", "www/llms.txt"] {
        let text = read(file);
        for key in &want {
            assert!(text.contains(key.as_str()), "{file} never names the slo key `{key}`");
        }
        let found = slo_keys_in(&text);
        let extra: Vec<&String> = found.difference(&want).collect();
        assert!(extra.is_empty(), "{file} names slo keys the const does not have: {extra:?}");
        assert_eq!(found, want, "{file}: the `slo.<key>` references are exactly SLO_TARGETS");
    }
}

#[test]
fn slopg_ac04_the_scanner_catches_a_stray_key() {
    let found = slo_keys_in("see slo.availability_30d and slo.made_up_key, not unslo.x or slo.");
    let want: BTreeSet<String> = ["availability_30d", "made_up_key"].iter().map(|s| s.to_string()).collect();
    assert_eq!(found, want);
}

#[test]
fn slopg_ac04_status_html_spells_no_target_number() {
    // Joe changes a target in one Rust const; the page must not carry its own copy.
    let html = read("www/status.html");
    for t in &SLO_TARGETS {
        let spelled = format!("{}", t.target);
        let line = html
            .lines()
            .filter(|l| l.contains("slo."))
            .find(|l| l.contains(spelled.as_str()) && !l.trim_start().starts_with("//"));
        assert!(line.is_none(), "status.html spells target {spelled} on a slo line: {line:?}");
    }
}
