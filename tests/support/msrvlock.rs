//! PRD-mcphost-msrv-locked-required-check -- shared helpers for the
//! `msrvlock_ac*` tests. Lives in a subdirectory so cargo/`gen-test-suites.sh`
//! (which only consume top-level `tests/*.rs`) never treat it as a target.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Run `scripts/required-checks.sh <args>` from `cwd` with extra env.
pub fn run_script(cwd: &Path, args: &[&str], env: &[(&str, &Path)]) -> Output {
    let mut cmd = Command::new("bash");
    cmd.arg(repo_root().join("scripts/required-checks.sh"))
        .args(args)
        .current_dir(cwd);
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().expect("spawn scripts/required-checks.sh")
}

pub fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

/// A scratch directory unique to this process + tag, emptied on creation.
pub fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("msrvlock-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("create scratch dir");
    d
}

/// The context lines the committed toml / `--generate --print` carry.
pub fn toml_contexts(toml_text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_list = false;
    for line in toml_text.lines() {
        let t = line.trim();
        if t.starts_with("contexts") && t.contains('[') {
            in_list = true;
            continue;
        }
        if in_list {
            if t.starts_with(']') {
                break;
            }
            let t = t.trim_end_matches(',');
            if let Some(s) = t.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
                out.push(s.replace("\\\"", "\"").replace("\\\\", "\\"));
            }
        }
    }
    out
}

/// Copy `.github/workflows` into a fresh scratch dir and return its path.
pub fn copy_workflows(tag: &str) -> PathBuf {
    let dst = scratch(tag).join("workflows");
    std::fs::create_dir_all(&dst).expect("create workflows copy");
    let src = repo_root().join(".github/workflows");
    for e in std::fs::read_dir(src).expect("read workflows") {
        let e = e.expect("dir entry");
        std::fs::copy(e.path(), dst.join(e.file_name())).expect("copy workflow");
    }
    dst
}

/// `Ok` when `toml` lists exactly the contexts `--print` derives from
/// `workflows`; otherwise `Err` naming every differing context.
pub fn toml_equals_generate(workflows: &Path, toml: &Path) -> Result<(), String> {
    let out = run_script(
        &repo_root(),
        &["--print"],
        &[("WORKFLOWS_DIR", workflows)],
    );
    assert!(out.status.success(), "--print failed:\n{}", text(&out));
    let generated = toml_contexts(&String::from_utf8_lossy(&out.stdout));
    let committed = toml_contexts(&std::fs::read_to_string(toml).expect("read toml"));
    let mut diffs = Vec::new();
    for c in &generated {
        if !committed.contains(c) {
            diffs.push(format!("generated but not in toml: {c}"));
        }
    }
    for c in &committed {
        if !generated.contains(c) {
            diffs.push(format!("in toml but not generated: {c}"));
        }
    }
    if diffs.is_empty() {
        Ok(())
    } else {
        Err(diffs.join("\n"))
    }
}

pub const CONTEXTS: [&str; 11] = [
    "docs client blocks match docs/clients.toml",
    "docs external links resolve",
    "flake-lint (env-mutation, single-shot-perf, receipt-read)",
    "msrv check (builds with the declared rust-version toolchain)",
    "registry-manifest --check + schema validation",
    "sandbox shard plan",
    "sandbox suites (all shards)",
    "sandbox suites (user namespaces, shard 1/3)",
    "sandbox suites (user namespaces, shard 2/3)",
    "sandbox suites (user namespaces, shard 3/3)",
    "static analysis + core suites",
];
