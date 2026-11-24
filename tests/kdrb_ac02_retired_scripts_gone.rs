//! contract: reads scripts/generated-docs-check.sh
//! PRD-mcphost-kind-docs-from-registry-b-v2 AC2: the two regex doc-check
//! scripts are gone from `scripts/` and named by no file, and the CI
//! generator step runs `gen-docs kinds --check` and the renamed umbrella.

use std::path::{Path, PathBuf};

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// The retired names, spelled in two halves so this file does not name them.
fn retired() -> [String; 2] {
    [["sandbox-api", "-doc-check.sh"].concat(), ["spec-fields", "-doc-check.sh"].concat()]
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

#[test]
fn kdrb_ac02_retired_scripts_gone() {
    let names = retired();
    for name in &names {
        assert!(!root().join("scripts").join(name).exists(), "scripts/{name} still exists");
    }
    assert!(root().join("scripts/generated-docs-check.sh").is_file(), "the renamed umbrella is missing");

    let mut files = Vec::new();
    for dir in ["src", "tests", "scripts", "docs", "www", ".github", "agent"] {
        walk(&root().join(dir), &mut files);
    }
    let mut named = Vec::new();
    for file in files {
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
        for name in &names {
            // The scripts' own stem is the name without its `.sh`.
            let stem = name.trim_end_matches(".sh");
            if text.contains(stem) {
                named.push(format!("{} names {stem}", file.strip_prefix(root()).unwrap().display()));
            }
        }
    }
    assert!(named.is_empty(), "retired scripts still named: {named:?}");

    let ci = std::fs::read_to_string(root().join(".github/workflows/ci.yml")).expect("read ci.yml");
    let step_start = ci.find("name: every generator in check mode").expect("generator step");
    let step = &ci[step_start..];
    for cmd in ["check ./target/debug/mcphost gen-docs kinds --check", "check ./scripts/generated-docs-check.sh"] {
        assert!(step.contains(cmd), "the CI generator step lacks `{cmd}`");
    }
}
