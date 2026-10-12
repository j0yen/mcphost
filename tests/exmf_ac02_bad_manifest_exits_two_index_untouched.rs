//! AC2: a manifest with `kind = "pyton"` or a `file` that does not exist
//! makes `mcphost examples index` exit 2, naming directory, field and
//! reason, and `index.json` is not rewritten.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SENTINEL: &str = "{\"sentinel\": true}\n";

fn tree(tag: &str, kind: &str, file: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("exmf-ac02-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let dir = root.join("demo");
    std::fs::create_dir_all(dir.join("tools")).unwrap();
    std::fs::write(dir.join("proof.sh"), "#!/bin/sh\n").unwrap();
    std::fs::write(dir.join("tools/real.py"), "def main(args):\n    return {}\n").unwrap();
    std::fs::write(
        dir.join("example.toml"),
        format!(
            "[example]\nname = \"demo\"\ntitle = \"Demo\"\nsummary = \"A demo.\"\nuse_case = [\"chart\"]\nclients = [\"any\"]\nsince = \"0.1.0\"\n\n[[tools]]\nname = \"real\"\nkind = \"{kind}\"\nfile = \"{file}\"\n\n[first_call]\ntool = \"real\"\nexpect = \"result.a == 1\"\n"
        ),
    )
    .unwrap();
    std::fs::write(root.join("index.json"), SENTINEL).unwrap();
    root
}

fn index(root: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["examples", "index", "--dir"])
        .arg(root)
        .output()
        .expect("run mcphost examples index")
}

fn assert_refused(root: &Path, field: &str, reason: &str) {
    let out = index(root);
    assert_eq!(out.status.code(), Some(2), "stdout: {}", String::from_utf8_lossy(&out.stdout));
    let stderr = String::from_utf8_lossy(&out.stderr);
    for needle in ["demo", field, reason] {
        assert!(stderr.contains(needle), "stderr missing `{needle}`: {stderr}");
    }
    assert_eq!(std::fs::read_to_string(root.join("index.json")).unwrap(), SENTINEL, "index.json was rewritten");
}

#[test]
fn misspelled_kind_exits_two_and_leaves_index_alone() {
    let root = tree("kind", "pyton", "tools/real.py");
    assert_refused(&root, "tools[0].kind", "pyton");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn missing_tool_file_exits_two_and_leaves_index_alone() {
    let root = tree("file", "python", "tools/missing.py");
    assert_refused(&root, "tools[0].file", "tools/missing.py");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_valid_tree_does_rewrite_the_index() {
    let root = tree("ok", "python", "tools/real.py");
    let out = index(&root);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let written = std::fs::read_to_string(root.join("index.json")).unwrap();
    assert_ne!(written, SENTINEL);
    assert!(written.contains("\"name\": \"demo\""));
    let _ = std::fs::remove_dir_all(&root);
}
