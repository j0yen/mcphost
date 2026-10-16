//! PRD-mcphost-emitted-doc-pointers-resolve-b
//! AC3 (P1) — Given the section for an anchor exceeds 4 KB, When fetched,
//! Then `text` is cut at a paragraph boundary with `truncated: true` and
//! `line` points at the heading.

use mcphost::doc_anchors::{DocAnchor, SECTION_MAX_BYTES, help_get};
use mcphost::doc_scan::scan;
use serde_json::json;
use std::path::Path;

/// A section of 40-byte paragraphs, far past the cap.
fn long_doc() -> String {
    let mut doc = String::from("# Top\n\nintro\n\n## Long\n\n");
    for i in 0..300 {
        doc.push_str(&format!("paragraph {i:03} {}\n\n", "x".repeat(24)));
    }
    doc.push_str("## After\n\nshort\n");
    doc
}

#[test]
fn edpb_ac03_scan_cuts_a_long_section_at_a_paragraph_boundary() {
    let doc = long_doc();
    let long = scan(&doc).into_iter().find(|h| h.title == "Long").expect("heading");
    assert!(long.truncated);
    assert!(long.text.len() <= SECTION_MAX_BYTES, "{}", long.text.len());
    assert!(long.text.len() > SECTION_MAX_BYTES - 80, "cut far short of the cap: {}", long.text.len());
    // Whole paragraphs only: every kept paragraph is a complete source one.
    let n = long.text.split("\n\n").count();
    let want = (0..n).map(|i| format!("paragraph {i:03} {}", "x".repeat(24))).collect::<Vec<_>>().join("\n\n");
    assert_eq!(long.text, want, "text must end on a paragraph boundary");
    assert_eq!(long.line, doc.lines().position(|l| l == "## Long").unwrap() + 1, "line is the heading's");

    let short = scan(&doc).into_iter().find(|h| h.title == "After").unwrap();
    assert!(!short.truncated);
    assert_eq!(short.text, "short");
}

#[test]
fn edpb_ac03_every_oversize_section_is_flagged_and_help_get_reports_it() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut flagged = 0;
    for a in DocAnchor::ALL {
        let body = help_get(&json!({"anchor": a.as_str()})).expect("every anchor is fetchable");
        assert_eq!(body["truncated"], json!(a.truncated()), "{}", a.as_str());
        assert!(body["text"].as_str().unwrap().len() <= SECTION_MAX_BYTES, "{}", a.as_str());
        if a.truncated() {
            flagged += 1;
            // `line` is the heading line in the source file.
            let src = std::fs::read_to_string(root.join(a.path())).unwrap();
            let heading = src.lines().nth(a.line() as usize - 1).unwrap();
            assert!(heading.starts_with('#') && heading.contains(a.title()), "{}: line {} is {heading:?}", a.as_str(), a.line());
            assert_eq!(body["line"], json!(a.line()));
            assert!(a.section_text().is_char_boundary(a.section_text().len()));
        }
    }
    println!("{flagged} truncated sections");
    assert!(flagged > 0, "the repo's docs have at least one section over 4 KB");
}

#[test]
fn edpb_ac03_short_section_is_not_truncated() {
    let body = help_get(&json!({"anchor": "README.md#call-limits"})).unwrap();
    assert_eq!(body["truncated"], json!(false));
}
