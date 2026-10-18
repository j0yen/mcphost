//! Shared helpers for the `tssdk_ac*` proofs (PRD-mcphost-typescript-sdk-generated):
//! run `npm` inside `sdk/ts`, one at a time. Every proof shares the one
//! `sdk/ts/node_modules` and `sdk/ts/dist`, and two concurrent `npm`s would
//! race installing / building them.
//!
//! `allow(dead_code)`: each including test file uses only part of this.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Mutex;

static NPM_LOCK: Mutex<()> = Mutex::new(());

pub fn sdk_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("sdk/ts")
}

fn run(args: &[&str]) -> Output {
    Command::new("npm")
        .args(args)
        .current_dir(sdk_dir())
        .output()
        .unwrap_or_else(|e| panic!("cannot run `npm {}`: {e}", args.join(" ")))
}

/// `npm <args>` in `sdk/ts`, after a one-time `npm ci` when `node_modules`
/// is missing; returns the raw `Output`.
pub fn npm(args: &[&str]) -> Output {
    let _guard = NPM_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if !sdk_dir().join("node_modules/.bin/tsc").exists() {
        let ci = run(&["ci", "--no-audit", "--no-fund"]);
        assert!(ci.status.success(), "npm ci failed:\n{}", text(&ci));
    }
    run(args)
}

pub fn text(out: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
}

/// `npm <args>` must exit 0; the failure message carries the output.
pub fn npm_ok(args: &[&str]) {
    let out = npm(args);
    assert!(out.status.success(), "npm {} failed:\n{}", args.join(" "), text(&out));
}

/// `npm test -- <file>`: one vitest file (path relative to `sdk/ts`).
pub fn vitest_ok(file: &str) {
    npm_ok(&["test", "--", file]);
}

/// Every file under `dir` (recursively).
pub fn package_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).expect("read package dir") {
        let p = entry.expect("dir entry").path();
        if p.is_dir() {
            out.extend(package_files(&p));
        } else {
            out.push(p);
        }
    }
    out.sort();
    out
}
