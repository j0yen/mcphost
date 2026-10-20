//! Shared fixture helpers for the PRD-mcphost-generated-code-map `cmap_ac*` tests.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

pub fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

pub fn script() -> PathBuf {
    repo().join("scripts/gen-codemap.sh")
}

/// A fresh empty scratch directory under the system temp dir.
pub fn scratch(label: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "mcphost-cmap-{label}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

pub fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
    fs::write(p, body).expect("write fixture file");
}

fn git(root: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .expect("run git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

/// Make `root` a git repo and stage everything, so `git ls-files` sees it.
pub fn git_stage(root: &Path) {
    if !root.join(".git").exists() {
        git(root, &["init", "-q"]);
    }
    git(root, &["add", "-A"]);
}

/// A fixture repo holding `files` (path, body), staged.
pub fn fixture(label: &str, files: &[(&str, String)]) -> PathBuf {
    let root = scratch(label);
    for (rel, body) in files {
        write(&root, rel, body);
    }
    git_stage(&root);
    root
}

fn copy_rs(from: &Path, to: &Path) {
    for e in fs::read_dir(from).expect("read_dir") {
        let p = e.expect("entry").path();
        let dest = to.join(p.file_name().expect("name"));
        if p.is_dir() {
            fs::create_dir_all(&dest).expect("mkdir");
            copy_rs(&p, &dest);
        } else if p.extension().is_some_and(|x| x == "rs") {
            fs::copy(&p, &dest).expect("copy");
        }
    }
}

/// A staged fixture repo carrying a copy of the real `src/**/*.rs` tree
/// (a filesystem copy, so it works in a checkout with no `.git`).
pub fn real_src_copy(label: &str) -> PathBuf {
    let root = scratch(label);
    fs::create_dir_all(root.join("src")).expect("mkdir");
    copy_rs(&repo().join("src"), &root.join("src"));
    git_stage(&root);
    root
}

/// Every `.rs` path under `root/src`, relative, sorted.
pub fn src_paths(root: &Path) -> Vec<String> {
    fn walk(dir: &Path, base: &Path, out: &mut Vec<String>) {
        for e in fs::read_dir(dir).expect("read_dir") {
            let p = e.expect("entry").path();
            if p.is_dir() {
                walk(&p, base, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p.strip_prefix(base).expect("prefix").to_string_lossy().into_owned());
            }
        }
    }
    let mut out = Vec::new();
    walk(&root.join("src"), root, &mut out);
    out.sort();
    out
}

/// Run the generator against `root`, optionally with `--check`.
pub fn run(root: &Path, check: bool) -> Output {
    let mut cmd = Command::new("bash");
    cmd.arg(script()).env("CODEMAP_ROOT", root);
    if check {
        cmd.arg("--check");
    }
    cmd.output().expect("run gen-codemap.sh")
}

pub fn codemap(root: &Path) -> String {
    fs::read_to_string(root.join("CODEMAP.md")).expect("read CODEMAP.md")
}

/// The `## <heading>` section body of a CODEMAP.md.
pub fn section(map: &str, heading: &str) -> String {
    let head = format!("## {heading}");
    let mut it = map.lines().skip_while(|l| *l != head);
    it.next();
    it.take_while(|l| !l.starts_with("## ")).collect::<Vec<_>>().join("\n")
}

/// A minimal source file with a `//!` doc.
pub fn src_file(doc: &str) -> String {
    format!("//! {doc}\n\npub fn f() {{}}\n")
}
