//! AC1 — Given `docs/kinds/wasm.md` with one `## Contract` row hand-edited,
//! When `scripts/kind-contract-doc-check.sh` runs, Then it exits 1 naming
//! `wasm` and the row; after `gendocs --write` it exits 0.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn check(root: &Path) -> Output {
    Command::new("bash")
        .arg(crate_root().join("scripts/kind-contract-doc-check.sh"))
        .arg(root)
        .env("MCPHOST_BIN", env!("CARGO_BIN_EXE_mcphost"))
        .output()
        .expect("run kind-contract-doc-check.sh")
}

#[test]
fn kconb_ac01_hand_edited_contract_row_fails_then_gendocs_write_fixes() {
    let tmp = std::env::temp_dir().join(format!("kconb_ac01_{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(tmp.join("docs/kinds")).unwrap();
    for entry in fs::read_dir(crate_root().join("docs/kinds")).unwrap() {
        let path = entry.unwrap().path();
        fs::copy(&path, tmp.join("docs/kinds").join(path.file_name().unwrap())).unwrap();
    }

    let clean = check(&tmp);
    assert!(clean.status.success(), "committed docs are in sync: {}", String::from_utf8_lossy(&clean.stderr));

    let wasm_md = tmp.join("docs/kinds/wasm.md");
    let original = "| budget `timeout_s` | 1-30 (default 5) |";
    let doc = fs::read_to_string(&wasm_md).unwrap();
    assert!(doc.contains(original), "wasm.md carries the generated timeout row");
    fs::write(&wasm_md, doc.replace(original, "| budget `timeout_s` | 1-99 (default 5) |")).unwrap();

    let drifted = check(&tmp);
    assert_eq!(drifted.status.code(), Some(1), "a hand-edited row exits 1");
    let stderr = String::from_utf8_lossy(&drifted.stderr);
    assert!(stderr.contains("kind=wasm"), "names the kind: {stderr}");
    assert!(stderr.contains(original), "names the row: {stderr}");

    let write = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["gen-docs", "--kinds-only"])
        .current_dir(&tmp)
        .output()
        .expect("run gen-docs --kinds-only");
    assert!(write.status.success(), "gen-docs --kinds-only writes: {}", String::from_utf8_lossy(&write.stderr));
    let fixed = check(&tmp);
    assert!(fixed.status.success(), "after gendocs --write it exits 0: {}", String::from_utf8_lossy(&fixed.stderr));
    let _ = fs::remove_dir_all(&tmp);
}
