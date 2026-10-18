//! Shared helpers for the `pysdk_ac*` proofs (PRD-mcphost-python-sdk-generated):
//! run `uv` inside `sdk/python`, one at a time. Every proof shares the one
//! `sdk/python/.venv`, and two concurrent `uv run`s would race creating it.
//!
//! `allow(dead_code)`: each including test file uses only part of this.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Mutex;

static UV_LOCK: Mutex<()> = Mutex::new(());

pub fn sdk_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("sdk/python")
}

/// `uv <args>` in `sdk/python`; panics with the full output when `uv` is
/// missing, returns the raw `Output` otherwise.
pub fn uv(args: &[&str]) -> Output {
    let _guard = UV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    Command::new("uv")
        .args(args)
        .current_dir(sdk_dir())
        .output()
        .unwrap_or_else(|e| panic!("cannot run `uv {}`: {e}", args.join(" ")))
}

pub fn text(out: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
}

/// `uv run pytest <path>` must exit 0; the failure message carries the output.
pub fn pytest_ok(path: &str) {
    let out = uv(&["run", "pytest", path, "-q"]);
    assert!(out.status.success(), "uv run pytest {path} failed:\n{}", text(&out));
}

/// Every file under `dir` (recursively), skipping caches.
pub fn package_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).expect("read package dir") {
        let p = entry.expect("dir entry").path();
        if p.file_name().is_some_and(|n| n == "__pycache__") {
            continue;
        }
        if p.is_dir() {
            out.extend(package_files(&p));
        } else {
            out.push(p);
        }
    }
    out.sort();
    out
}
