//! PRD-mcphost-delegation-recipes-and-quickstart
//! AC7 (P0) — Given `grep -rn 'scope_escalation\|key_revoked' www README.md`,
//! When run after landing, Then every hit lies inside a generated marker block.

use mcphost::llms_txt::{RECIPE_END, RECIPE_START};
use std::path::{Path, PathBuf};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read_dir") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            files_under(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// Every `(line number, line)` of `text` naming either code outside the
/// recipe marker block (markers inclusive).
fn stray_hits(text: &str) -> Vec<(usize, String)> {
    let mut inside = false;
    let mut hits = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.contains(RECIPE_START) {
            inside = true;
        }
        if !inside && (line.contains("scope_escalation") || line.contains("key_revoked")) {
            hits.push((i + 1, line.to_string()));
        }
        if line.contains(RECIPE_END) {
            inside = false;
        }
    }
    hits
}

#[test]
fn dlrec_ac07_every_hit_in_www_and_readme_is_inside_a_generated_block() {
    let root = Path::new(ROOT);
    let mut files = vec![root.join("README.md")];
    files_under(&root.join("www"), &mut files);
    let mut total = 0;
    for f in files {
        let Ok(text) = std::fs::read_to_string(&f) else { continue };
        total += text.matches("scope_escalation").count() + text.matches("key_revoked").count();
        let stray = stray_hits(&text);
        assert!(stray.is_empty(), "{}: hand-written mention outside a recipe block: {stray:?}", f.display());
    }
    assert!(total >= 2, "the generated blocks themselves carry the codes");
}

#[test]
fn dlrec_ac07_the_scan_catches_a_hand_written_mention() {
    let text = format!("intro\nfails `scope_escalation` by hand\n{RECIPE_START}\n`key_revoked`\n{RECIPE_END}\n");
    assert_eq!(stray_hits(&text), vec![(2, "fails `scope_escalation` by hand".to_string())]);
}
