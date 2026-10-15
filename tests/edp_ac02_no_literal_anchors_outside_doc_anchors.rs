//! PRD-mcphost-emitted-doc-pointers-resolve
//! AC2 (P0) — Given `src/**/*.rs` after the change, When the literal scan
//! runs, Then it finds zero `.md#` strings outside `doc_anchors.rs` and
//! `DocAnchor` has one variant for every heading in the scanned files.

use mcphost::doc_anchors::DocAnchor;
use mcphost::doc_scan;
use std::path::{Path, PathBuf};

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read_dir").filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// `[A-Za-z/._-]+\.md#[a-z0-9-]+` matches in `line`, without a regex crate.
fn literal_hits(line: &str) -> Vec<String> {
    let path_char = |c: char| c.is_ascii_alphabetic() || matches!(c, '/' | '.' | '_' | '-');
    let slug_char = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-';
    let mut hits = Vec::new();
    let mut from = 0;
    while let Some(i) = line[from..].find(".md#") {
        let at = from + i;
        let head: String = line[..at].chars().rev().take_while(|c| path_char(*c)).collect::<Vec<_>>().into_iter().rev().collect();
        let tail: String = line[at + 4..].chars().take_while(|c| slug_char(*c)).collect();
        if !head.is_empty() && !tail.is_empty() {
            hits.push(format!("{head}.md#{tail}"));
        }
        from = at + 4;
    }
    hits
}

#[test]
fn edp_ac02_zero_md_anchor_literals_outside_doc_anchors_rs() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);
    assert!(files.len() > 50, "scan found only {} files", files.len());
    let mut found = Vec::new();
    for file in files.iter().filter(|f| f.file_name().is_some_and(|n| n != "doc_anchors.rs")) {
        let text = std::fs::read_to_string(file).expect("read");
        for (n, line) in text.lines().enumerate() {
            for hit in literal_hits(line) {
                found.push(format!("{}:{} {hit}", file.strip_prefix(root).unwrap().display(), n + 1));
            }
        }
    }
    assert!(found.is_empty(), "`.md#` literals outside src/doc_anchors.rs:\n{}", found.join("\n"));
}

#[test]
fn edp_ac02_scanner_catches_a_literal() {
    assert_eq!(literal_hits(r#"x: "README.md#call-limits","#), vec!["README.md#call-limits"]);
    assert!(literal_hits("no anchor here, only .md# alone").is_empty());
}

#[test]
fn edp_ac02_doc_anchor_has_one_variant_per_heading() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = vec!["README.md".to_string(), "www/llms.txt".to_string(), "www/llms-install.md".to_string()];
    let mut stack = vec![root.join("docs")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read_dir").filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "md") {
                files.push(path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut expected = Vec::new();
    for rel in &files {
        let text = std::fs::read_to_string(root.join(rel)).expect("read doc");
        for h in doc_scan::scan(&text) {
            expected.push(format!("{rel}#{}", h.slug));
        }
    }
    expected.sort();
    let mut got: Vec<String> = DocAnchor::ALL.iter().map(|a| a.as_str().to_string()).collect();
    got.sort();
    assert_eq!(got, expected, "DocAnchor must have exactly one variant per scanned heading");
    let mut unique = got.clone();
    unique.dedup();
    assert_eq!(unique.len(), got.len(), "anchors are unique");
    for a in DocAnchor::ALL {
        assert_eq!(DocAnchor::parse(a.as_str()), Some(*a));
    }
}
