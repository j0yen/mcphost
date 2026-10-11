//! PRD-mcphost-blob-virus-and-type-guard AC1 — the benign fixture corpus
//! (30 files) is run through the allowlist with the default `accept`; the
//! refusal count is printed and must be 0. The malicious twin proves the
//! corpus is not vacuous: the same guard refuses a PE and a bomb.
use crate::blobguard;

use mcphost::blobs::{check, Policy};

#[test]
fn benign_corpus_has_zero_refusals_under_default_accept() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/blobguard/benign");
    let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()).collect();
    files.sort();
    assert_eq!(files.len(), 30, "corpus is 30 files");
    let mut refused = Vec::new();
    for f in &files {
        let name = f.file_name().unwrap().to_str().unwrap();
        let bytes = std::fs::read(f).unwrap();
        if let Err(r) = check(name, &bytes, &Policy::default(), None) {
            refused.push(format!("{name}: {} ({})", r.reason, r.detail));
        }
    }
    println!("benign refusals: {}/{}", refused.len(), files.len());
    for r in &refused {
        println!("  refused {r}");
    }
    assert!(refused.is_empty(), "benign files refused: {refused:?}");
}

#[test]
fn the_same_guard_refuses_malicious_fixtures() {
    let p = Policy::default();
    assert!(check("a.exe", blobguard::PE, &p, None).is_err());
    assert!(check("data.csv", blobguard::PE, &p, None).is_err());
    let bomb = blobguard::zip(&[("a.csv", b"x,y\n", Some(u32::MAX))]);
    assert_eq!(check("b.zip", &bomb, &p, None).unwrap_err().reason, "archive_ratio");
    assert_eq!(check("v.svg", b"<svg><script>alert(1)</script></svg>", &p, None).unwrap_err().reason, "active_content");
}
