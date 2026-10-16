//! PRD-mcphost-emitted-doc-pointers-resolve-b
//! AC1 (P0) — Given a README heading renamed in a test copy of the repo,
//! When `mcphost doc-anchors --check` runs, Then it exits 1 naming the
//! stale pointer and the file:line that emits it.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A scratch repo: README.md, `docs/` and `www/` copies plus `src/*.rs`.
struct Copy {
    dir: PathBuf,
}

fn copy_tree(from: &Path, to: &Path, ext: Option<&str>) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().filter_map(|e| e.ok()) {
        let p = e.path();
        if p.is_dir() {
            copy_tree(&p, &to.join(e.file_name()), ext);
        } else if ext.is_none_or(|x| p.extension().is_some_and(|e| e == x)) {
            std::fs::copy(&p, to.join(e.file_name())).unwrap();
        }
    }
}

impl Copy {
    fn new(tag: &str) -> Copy {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let dir = std::env::temp_dir().join(format!(
            "mcphost-edpb-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
        ));
        copy_tree(&root.join("src"), &dir.join("src"), Some("rs"));
        copy_tree(&root.join("docs"), &dir.join("docs"), Some("md"));
        std::fs::create_dir_all(dir.join("www")).unwrap();
        for f in ["README.md", "www/llms.txt", "www/llms-install.md"] {
            std::fs::copy(root.join(f), dir.join(f)).unwrap();
        }
        Copy { dir }
    }

    fn check(&self) -> Output {
        Command::new(env!("CARGO_BIN_EXE_mcphost"))
            .args(["doc-anchors", "--check", "--root"])
            .arg(&self.dir)
            .output()
            .expect("run mcphost doc-anchors")
    }
}

#[test]
fn edpb_ac01_unmodified_copy_is_green_with_zero_unresolved() {
    let c = Copy::new("green");
    let out = c.check();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("unresolved=0"), "{stdout}");
    assert!(stdout.starts_with("anchors="), "{stdout}");
}

#[test]
fn edpb_ac01_renamed_readme_heading_exits_one_naming_pointer_and_emitting_line() {
    let c = Copy::new("renamed");
    let readme = std::fs::read_to_string(c.dir.join("README.md")).unwrap();
    let renamed = readme.replacen("## Call limits", "## Call budgets", 1);
    assert_ne!(readme, renamed, "fixture edit must rename the heading");
    std::fs::write(c.dir.join("README.md"), renamed).unwrap();

    // Where help.rs emits it, read off the copy.
    let help = std::fs::read_to_string(c.dir.join("src/help.rs")).unwrap();
    let line = help.lines().position(|l| l.contains("DocAnchor::ReadmeCallLimits")).expect("emitting line") + 1;

    let out = c.check();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("README.md#call-limits"), "{stderr}");
    assert!(stderr.contains(&format!("src/help.rs:{line}")), "want src/help.rs:{line} in {stderr}");
    assert!(String::from_utf8_lossy(&out.stdout).contains("unresolved=1"));
}
