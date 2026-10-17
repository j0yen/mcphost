//! contract: no "insufficient_scope" literal appears in src outside src/scopes.rs, and the registry gate matches the legacy gate.
//! PRD-mcphost-scope-registry-generated
//! AC1 (P0) — Given the OAuth and key test corpus, When the registry replaces
//! the literals, Then no `"insufficient_scope"` literal remains in `src`
//! outside `src/scopes.rs`, and the registry's gate keeps every allowed and
//! refused outcome the old literal-based gate produced.

use mcphost::scopes::{self, Scope};
use std::path::Path;

fn rust_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read_dir") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn sreg_ac01_no_insufficient_scope_literal_outside_registry() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    let needle = format!("\"{}\"", scopes::INSUFFICIENT_SCOPE);
    let mut offenders = Vec::new();
    for f in files {
        if f.ends_with("scopes.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&f).expect("read");
        for (i, line) in text.lines().enumerate() {
            if line.contains(&needle) {
                offenders.push(format!("{}:{}", f.display(), i + 1));
            }
        }
    }
    assert!(offenders.is_empty(), "literal outside src/scopes.rs: {offenders:?}");
}

/// The pre-registry gate, verbatim: a held word satisfies a required word
/// only by equality, and `mcp` satisfies everything.
fn legacy(token: Option<&str>, required: &[&str]) -> bool {
    let Some(scope) = token else { return true };
    let words: Vec<&str> = scope.split_whitespace().collect();
    if words.contains(&"mcp") {
        return true;
    }
    let req: Vec<&str> = if required.is_empty() { vec!["mcp"] } else { required.to_vec() };
    req.iter().all(|r| words.contains(r))
}

#[test]
fn sreg_ac01_gate_matches_legacy_on_legacy_vocabulary() {
    let tokens = [None, Some("mcp"), Some("read"), Some("write"), Some("read write"), Some("billing"), Some("")];
    let reqs: [&[&str]; 6] = [&[], &["read"], &["write"], &["read", "write"], &["billing"], &["mcp"]];
    for t in tokens {
        for r in reqs {
            let owned: Vec<String> = r.iter().map(|s| s.to_string()).collect();
            assert_eq!(scopes::check(t, &owned), legacy(t, r), "token={t:?} required={r:?}");
        }
    }
}

#[test]
fn sreg_ac01_registry_names_are_unique_and_round_trip() {
    let names = scopes::all_names();
    let mut dedup = names.clone();
    dedup.sort();
    dedup.dedup();
    assert_eq!(dedup.len(), names.len());
    for s in Scope::ALL {
        assert_eq!(Scope::parse(s.as_str()), Some(*s));
    }
}
