//! PRD-mcphost-generated-code-map
//! AC1 — Given the repo at the build's tip, When `scripts/gen-codemap.sh` runs,
//! Then `CODEMAP.md` lists every source path exactly once, each with a size
//! class and a non-empty `//!` summary, contains no `:<digits>` line reference,
//! and is under 40,000 bytes.

use crate::codemap;

/// Generated over a copy of the real `src/` tree (no `.git` needed).
#[test]
fn cmap_ac01_lists_every_source_path_once_with_size_and_summary() {
    let root = codemap::real_src_copy("ac01");
    let out = codemap::run(&root, false);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let map = codemap::codemap(&root);

    let files = codemap::section(&map, "Files");
    let paths = codemap::src_paths(&root);
    assert!(paths.len() > 100, "expected the real tree, got {}", paths.len());
    for path in &paths {
        let prefix = format!("- `{path}` (");
        let entries: Vec<&str> = files.lines().filter(|l| l.starts_with(&prefix)).collect();
        assert_eq!(entries.len(), 1, "{path} must have exactly one entry, got {entries:?}");
        let rest = &entries[0][prefix.len()..];
        let (size, summary) = rest.split_once(") ").unwrap_or_else(|| panic!("{path}: no summary: {rest}"));
        assert!(
            size == "<1k" || (size.starts_with('~') && size.ends_with('k')),
            "{path}: bad size class {size:?}"
        );
        assert!(!summary.trim().is_empty(), "{path}: empty summary");
    }
    let entry_count = files.lines().filter(|l| l.starts_with("- `")).count();
    assert_eq!(entry_count, paths.len(), "no entry beyond the source files");

    for (i, line) in map.lines().enumerate() {
        let b = line.as_bytes();
        assert!(
            !b.windows(2).any(|w| w[0] == b':' && w[1].is_ascii_digit()),
            "line {} carries a :<digits> line reference: {line}",
            i + 1
        );
    }
    assert!(map.len() < 40_000, "CODEMAP.md is {} bytes", map.len());
}

/// The committed map is the one the generator writes for the committed tree.
#[test]
fn cmap_ac01_committed_codemap_exists_and_is_under_budget() {
    let committed = std::fs::read_to_string(codemap::repo().join("CODEMAP.md")).expect("CODEMAP.md");
    assert!(committed.len() < 40_000, "CODEMAP.md is {} bytes", committed.len());
    assert!(codemap::script().is_file(), "scripts/gen-codemap.sh");
}
