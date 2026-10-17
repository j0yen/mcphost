//! PRD-mcphost-retry-policy-table-generated AC6 (R4) — Given a hand edit to
//! the `## Retry policy` section of `www/llms.txt`, When `mcphost
//! retry-policy --check` runs, Then it exits 1 naming the file.
//!
//! Runs the real binary over a scratch copy of the three generated files so
//! the working tree is never touched.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn new() -> Scratch {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let dir = std::env::temp_dir().join(format!(
            "mcphost-rpol-ac06-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
        ));
        std::fs::create_dir_all(&dir).unwrap();
        for (from, to) in [
            ("www/llms.txt", "llms.txt"),
            ("docs/errors.md", "errors.md"),
            ("www/retry_policy.json", "retry_policy.json"),
        ] {
            std::fs::copy(root.join(from), dir.join(to)).unwrap();
        }
        Scratch { dir }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    fn run(&self, check: bool) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mcphost"));
        cmd.arg("retry-policy");
        if check {
            cmd.arg("--check");
        }
        cmd.arg("--llms").arg(self.path("llms.txt"));
        cmd.arg("--errors-doc").arg(self.path("errors.md"));
        cmd.arg("--json").arg(self.path("retry_policy.json"));
        cmd.output().expect("run mcphost retry-policy")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _: &Path = &self.dir;
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn rpol_ac06() {
    let s = Scratch::new();
    let doc = std::fs::read_to_string(s.path("llms.txt")).unwrap();
    assert!(doc.contains("## Retry policy"), "the committed llms.txt carries the section");
    let ok = s.run(true);
    assert_eq!(ok.status.code(), Some(0), "committed files are current: {}", String::from_utf8_lossy(&ok.stderr));

    // A hand edit inside the section: flip one row's retryable cell.
    let drifted = doc.replacen("| `args_invalid` | no |", "| `args_invalid` | yes |", 1);
    assert_ne!(drifted, doc, "edit must change the file");
    std::fs::write(s.path("llms.txt"), &drifted).unwrap();
    let stale = s.run(true);
    assert_eq!(stale.status.code(), Some(1), "{}", String::from_utf8_lossy(&stale.stderr));
    let err = String::from_utf8_lossy(&stale.stderr);
    assert!(err.contains("llms.txt"), "must name the file: {err}");
    assert!(!err.contains("errors.md") && !err.contains("retry_policy.json"), "only the drifted file: {err}");
    assert_eq!(std::fs::read_to_string(s.path("llms.txt")).unwrap(), drifted, "--check writes nothing");

    let regen = s.run(false);
    assert_eq!(regen.status.code(), Some(0), "{}", String::from_utf8_lossy(&regen.stderr));
    assert_eq!(std::fs::read_to_string(s.path("llms.txt")).unwrap(), doc, "regeneration restores the section");
    assert_eq!(s.run(true).status.code(), Some(0));
}
