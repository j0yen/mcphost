//! PRD-mcphost-msrv-locked-required-check AC1 -- a stale `Cargo.lock` fails a
//! core-suite test. Leg 20: PR #178's land squash dropped `semver` from the
//! lock; only the optional `msrv` job noticed. `cargo tree --locked --offline`
//! is the cheapest "lock matches manifest" probe (no network, no compile).

use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::{Command, Output};

use crate::msrvlock;
use msrvlock as support;

fn cargo_tree_locked(dir: &Path) -> Output {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    Command::new(cargo)
        .args(["tree", "--locked", "--offline", "--workspace", "-q"])
        .current_dir(dir)
        .output()
        .expect("spawn cargo tree")
}

/// Drop every `[[package]]` block named `name` from a Cargo.lock text.
fn lock_without(lock: &str, name: &str) -> String {
    let needle = format!("name = \"{name}\"");
    let mut blocks: Vec<&str> = lock.split("\n[[package]]\n").collect();
    let head = blocks.remove(0);
    let mut out = String::from(head);
    for b in blocks {
        if b.lines().next().map(str::trim) != Some(needle.as_str()) {
            out.push_str("\n[[package]]\n");
            out.push_str(b);
        }
    }
    out
}

#[test]
fn msrvlock_ac01_repo_lock_is_consistent_with_the_manifest() {
    let out = cargo_tree_locked(&support::repo_root());
    assert!(
        out.status.success(),
        "Cargo.lock is stale against the manifest (`cargo tree --locked --offline` failed):\n{}",
        support::text(&out)
    );
}

#[test]
fn msrvlock_ac01_lock_without_semver_fails_quoting_cannot_update() {
    let root = support::repo_root();
    let scratch = support::scratch("ac01");
    // Scratch copy of the workspace: every top-level entry symlinked except
    // the lock (rewritten) and build output / VCS state (not needed).
    for entry in std::fs::read_dir(&root).expect("read repo root") {
        let entry = entry.expect("dir entry");
        let name = entry.file_name();
        if matches!(name.to_str(), Some("Cargo.lock" | "target" | ".git")) {
            continue;
        }
        symlink(entry.path(), scratch.join(&name)).expect("symlink workspace entry");
    }
    let lock = std::fs::read_to_string(root.join("Cargo.lock")).expect("read Cargo.lock");
    let stale = lock_without(&lock, "semver");
    assert!(
        stale.len() < lock.len(),
        "fixture invariant: Cargo.lock must contain a `semver` package to remove"
    );
    std::fs::write(scratch.join("Cargo.lock"), stale).expect("write stale lock");

    let out = cargo_tree_locked(&scratch);
    let text = support::text(&out);
    let _ = std::fs::remove_dir_all(&scratch);
    assert!(
        !out.status.success(),
        "a lock missing `semver` must fail `cargo tree --locked --offline`:\n{text}"
    );
    assert!(
        text.contains("cannot update the lock file"),
        "failure must quote `cannot update the lock file` (the #178 line); got:\n{text}"
    );
}
