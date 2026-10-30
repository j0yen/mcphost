//! `mcphost contract dump` refuses an unparsable committed contract.
//!
//! `since_map` returns an empty map for text that does not parse, so a
//! committed `contracts/host-tools.v1.json` holding `<<<<<<<` conflict markers
//! (a land rebase committed exactly that) made `contract dump` silently
//! rewrite every tool's `since` to the current version. Now it exits 2 naming
//! the file and writes nothing; a valid or missing file behaves as before.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const COMMITTED: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/contracts/host-tools.v1.json");

/// A fresh empty directory under the system temp dir, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("mcphost-cdump-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        Scratch(dir)
    }
    fn file(&self) -> PathBuf {
        self.0.join("host-tools.v1.json")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn dump(path: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["contract", "dump", "--path"])
        .arg(path)
        .output()
        .expect("run mcphost contract dump")
}

#[test]
fn conflict_markers_in_the_committed_contract_are_refused_and_the_file_untouched() {
    let scratch = Scratch::new("markers");
    let planted = "{\n  \"contract_version\": 1,\n<<<<<<< HEAD\n  \"tools\": []\n=======\n  \"tools\": [{\"name\": \"x\"}]\n>>>>>>> main\n}\n".to_string();
    std::fs::write(scratch.file(), &planted).expect("plant markers");

    let out = dump(&scratch.file());

    assert_eq!(out.status.code(), Some(2), "must exit 2; stdout {:?}", String::from_utf8_lossy(&out.stdout));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("contract dump:")
            && stderr.contains(&scratch.file().display().to_string())
            && stderr.contains("does not parse")
            && stderr.contains("resolve it (take main's version) before regenerating"),
        "stderr must name the file and the remedy: {stderr}"
    );
    assert_eq!(std::fs::read_to_string(scratch.file()).expect("read back"), planted, "file must be untouched");
}

#[test]
fn a_contract_without_a_tools_array_is_refused_and_the_file_untouched() {
    let scratch = Scratch::new("notools");
    std::fs::write(scratch.file(), "{\"contract_version\": 1}\n").expect("plant");

    let out = dump(&scratch.file());

    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no tools array"));
    assert_eq!(std::fs::read_to_string(scratch.file()).unwrap(), "{\"contract_version\": 1}\n");
}

#[test]
fn a_valid_committed_contract_dumps_unchanged_with_every_since_kept() {
    let scratch = Scratch::new("valid");
    let committed = std::fs::read(COMMITTED).expect("read repo contract");
    std::fs::write(scratch.file(), &committed).expect("copy contract");

    let out = dump(&scratch.file());

    assert_eq!(out.status.code(), Some(0), "stderr {:?}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("contract dump: wrote"));
    assert_eq!(
        std::fs::read(scratch.file()).unwrap(),
        committed,
        "a valid contract regenerates byte-identical (every `since` preserved)"
    );
}

#[test]
fn a_missing_contract_file_is_still_written() {
    let scratch = Scratch::new("missing");
    assert!(!scratch.file().exists());

    let out = dump(&scratch.file());

    assert_eq!(out.status.code(), Some(0), "stderr {:?}", String::from_utf8_lossy(&out.stderr));
    let written = std::fs::read(scratch.file()).expect("dump wrote the file");
    assert!(serde_json::from_slice::<serde_json::Value>(&written).expect("valid json")["tools"].is_array());
}
