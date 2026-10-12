//! PRD-mcphost-blob-put-get-signed-url
//! AC1 — Given the synthorg corpus `corpora/mcphost/consumer-tasks.yaml`,
//! When a test greps for a task whose fixture is a binary file (`.pdf`,
//! `.png`, `.xlsx`, `.zip`), Then the count is printed; if 0 the builder adds
//! `recipe_file_share` (R7) in the same PR and the test passes with 1.
//!
//! The synthorg repo is not in this worktree, so the real corpus is vendored
//! at `tests/fixtures/blobput/synthorg-consumer-tasks.yaml`: its first
//! `upstream_bytes` bytes are synthorg's file at the commit named in
//! `synthorg-consumer-tasks.source`, pinned by sha256, and the tail is the
//! `recipe_file_share` block. Upstream alone counts 0 (asserted below), so
//! the 1 comes from the added task. Set `SYNTHORG_CORPUS` to a synthorg
//! checkout's own corpus file to run the same grep against it.

use sha2::{Digest, Sha256};
use std::path::Path;

const BINARY_EXTS: [&str; 4] = [".pdf", ".png", ".xlsx", ".zip"];

/// Ids of the tasks whose `fixture:` names a binary file.
fn binary_fixture_tasks(corpus: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut id = None;
    for line in corpus.lines() {
        let t = line.trim();
        if let Some(v) = t.strip_prefix("- id:") {
            id = Some(v.trim().to_string());
        } else if let Some(v) = t.strip_prefix("fixture:") {
            let v = v.trim().trim_matches('"');
            if BINARY_EXTS.iter().any(|e| v.ends_with(e)) {
                out.push((id.clone().expect("fixture outside a task"), v.to_string()));
            }
        }
    }
    out
}

fn source_field<'a>(source: &'a str, key: &str) -> &'a str {
    source
        .lines()
        .filter(|l| !l.starts_with('#'))
        .find_map(|l| l.strip_prefix(key)?.trim().strip_prefix('='))
        .map(str::trim)
        .unwrap_or_else(|| panic!("{key} missing from synthorg-consumer-tasks.source"))
}

#[test]
fn synthorg_corpus_has_exactly_one_binary_fixture_task_recipe_file_share() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let corpus_path = std::env::var_os("SYNTHORG_CORPUS")
        .map(Into::into)
        .unwrap_or_else(|| root.join("tests/fixtures/blobput/synthorg-consumer-tasks.yaml"));
    let corpus = std::fs::read_to_string(&corpus_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", corpus_path.display()));

    let found = binary_fixture_tasks(&corpus);
    println!("binary-fixture task count: {} ({})", found.len(), corpus_path.display());
    assert_eq!(found.len(), 1, "expected recipe_file_share only, got {found:?}");
    assert_eq!(found[0].0, "recipe_file_share");
    assert_eq!(found[0].1, "tests/fixtures/blobput/report.pdf");

    // It is the real corpus, not a stub: the existing recipe tasks are there.
    for id in ["recipe_share_a_tool", "recipe_team_memory", "recipe_webhook_inbox", "install_sdk"] {
        assert!(corpus.contains(&format!("- id: {id}\n")), "{id} missing: not the synthorg corpus");
    }

    let fx = std::fs::read(root.join(&found[0].1)).expect("fixture exists");
    assert_eq!(fx.len(), 300 * 1024, "fixture is 300 KB");
    assert!(fx.starts_with(b"%PDF-"), "fixture is a PDF");
}

#[test]
fn vendored_corpus_is_upstream_verbatim_plus_recipe_file_share() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = root.join("tests/fixtures/blobput");
    let corpus = std::fs::read(dir.join("synthorg-consumer-tasks.yaml")).expect("vendored corpus");
    let source = std::fs::read_to_string(dir.join("synthorg-consumer-tasks.source")).expect("source");
    let n: usize = source_field(&source, "upstream_bytes").parse().expect("upstream_bytes");
    let (upstream, added) = corpus.split_at(n);

    let sha: String = Sha256::digest(upstream).iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(sha, source_field(&source, "upstream_sha256"), "upstream prefix drifted from synthorg");

    // Without the added task the real corpus has no binary-fixture task:
    // this is the 0 that obliged the builder to add one.
    let upstream = std::str::from_utf8(upstream).expect("utf8");
    let before = binary_fixture_tasks(upstream);
    println!("upstream binary-fixture task count: {}", before.len());
    assert!(before.is_empty(), "upstream already had {before:?}");

    // The added tail is exactly the block examples/file-share ships.
    let example = std::fs::read_to_string(root.join("examples/file-share/synthorg-task.yaml"))
        .expect("examples/file-share/synthorg-task.yaml");
    let block = example.split_once("\ntasks:\n").expect("tasks: list").1;
    assert_eq!(std::str::from_utf8(added).expect("utf8"), format!("\n{block}"));
}
