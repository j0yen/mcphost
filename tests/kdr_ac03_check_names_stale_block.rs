//! PRD-mcphost-kind-docs-from-registry AC3: a hand-edited `kind-errors`
//! contract: walks docs/kinds/
//! block fails `gen-docs kinds --check` naming page and block; `--write`
//! repairs it and is idempotent.

use std::path::Path;
use std::process::{Command, Output};

fn mcphost(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .arg("gen-docs")
        .arg("kinds")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap()
}

fn copy_pages(to: &Path) {
    let dir = to.join("docs/kinds");
    std::fs::create_dir_all(&dir).unwrap();
    for entry in std::fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/kinds")).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), dir.join(entry.file_name())).unwrap();
    }
}

#[test]
fn kdr_ac03_check_names_stale_block() {
    let tmp = std::env::temp_dir().join(format!("kdr_ac03_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    copy_pages(&tmp);
    assert!(mcphost(&tmp, &["--write"]).status.success());
    assert!(mcphost(&tmp, &["--check"]).status.success(), "fresh pages pass --check");

    let page = tmp.join("docs/kinds/http.md");
    let text = std::fs::read_to_string(&page).unwrap();
    let edited = text.replace("| `upstream_error` |", "| `upstream_error_hand_edit` |");
    assert_ne!(text, edited, "the edit must change the page");
    std::fs::write(&page, edited).unwrap();

    let out = mcphost(&tmp, &["--check"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("docs/kinds/http.md") && stderr.contains("kind-errors"), "{stderr}");
    assert_eq!(std::fs::read_to_string(&page).unwrap().contains("hand_edit"), true, "--check never writes");

    assert!(mcphost(&tmp, &["--write"]).status.success());
    assert!(mcphost(&tmp, &["--check"]).status.success());
    let first = std::fs::read_to_string(&page).unwrap();
    assert_eq!(first, text, "--write restores the generated block");
    assert!(mcphost(&tmp, &["--write"]).status.success());
    assert_eq!(std::fs::read_to_string(&page).unwrap(), first, "second --write changes nothing");
    let _ = std::fs::remove_dir_all(&tmp);
}
