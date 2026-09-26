//! PRD-mcphost-shared-tool-call-path
//! AC5 — Given `www/llms.txt` after regeneration, When
//! `scripts/gen-docs-sharing.sh --check` runs in CI, Then it exits 0; When a
//! line of the sharing section is hand-edited, Then it exits non-zero
//! naming the line.
//!
//! Runs the real script against a scratch copy of the three files it
//! touches (`scripts/gen-docs-sharing.sh`, `docs/sharing.md`,
//! `www/llms.txt`), never the repo's own on-disk copies -- a failing
//! `--check` run in this test must never leave the actual working tree
//! touched, and running two `#[test]`s that both hand-edit the real
//! `www/llms.txt` in place would race each other under `cargo test`'s
//! default parallelism.

use std::path::Path;
use std::process::Command;

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// A scratch copy of `scripts/gen-docs-sharing.sh` + `docs/sharing.md` +
/// `www/llms.txt`, laid out at the same relative paths so the script's own
/// `cd "$(dirname "$0")/.."` resolves correctly.
fn scratch_copy(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-sharedcall-ac05-{label}-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    for sub in ["scripts", "docs", "www"] {
        std::fs::create_dir_all(dir.join(sub)).expect("create scratch subdir");
    }
    for rel in ["scripts/gen-docs-sharing.sh", "docs/sharing.md", "www/llms.txt"] {
        std::fs::copy(repo_root().join(rel), dir.join(rel))
            .unwrap_or_else(|e| panic!("copy {rel} into scratch: {e}"));
    }
    dir
}

fn run_check(dir: &Path) -> std::process::Output {
    Command::new("bash")
        .arg(dir.join("scripts/gen-docs-sharing.sh"))
        .arg("--check")
        .output()
        .expect("run gen-docs-sharing.sh --check")
}

#[test]
fn check_passes_on_the_committed_files() {
    let dir = scratch_copy("clean");
    let output = run_check(&dir);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "gen-docs-sharing.sh --check must exit 0 on the committed files, stderr:\n{stderr}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn check_fails_naming_the_line_after_a_hand_edit() {
    let dir = scratch_copy("edited");
    let llms_path = dir.join("www/llms.txt");
    let original = std::fs::read_to_string(&llms_path).expect("read scratch llms.txt");
    let lines: Vec<&str> = original.lines().collect();
    let target_line_no = lines
        .iter()
        .position(|l| l.contains("Store the key once"))
        .expect("sharing section must still mention storing the key once")
        + 1; // 1-indexed for the assertion below

    let mut edited_lines: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    edited_lines[target_line_no - 1] = "HAND EDITED, MUST BE CAUGHT".to_string();
    std::fs::write(&llms_path, edited_lines.join("\n") + "\n").expect("write hand-edited llms.txt");

    let output = run_check(&dir);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "gen-docs-sharing.sh --check must exit non-zero after a hand edit"
    );
    assert!(
        stderr.contains(&format!("line {target_line_no}")),
        "stderr must name the edited line ({target_line_no}), got:\n{stderr}"
    );

    // --check must never leave a write behind: the hand-edited file must
    // still read back exactly as this test left it.
    let after = std::fs::read_to_string(&llms_path).expect("re-read scratch llms.txt");
    assert_eq!(after, edited_lines.join("\n") + "\n", "--check must not modify the file it's checking");

    std::fs::remove_dir_all(&dir).ok();
}
