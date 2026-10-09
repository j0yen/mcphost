//! Stamps `MCPHOST_GIT_SHA` (12-char short sha) into the build when a `.git`
//! is present; leaves it unset otherwise so `build_info::BUILD.git_sha` is
//! `None`. Never fails the build when `git` is absent.
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-env-changed=MCPHOST_GIT_SHA");
    if std::env::var_os("MCPHOST_GIT_SHA").is_some() {
        return;
    }
    if !std::path::Path::new(".git").exists() {
        return;
    }
    let Ok(out) = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
    else {
        return;
    };
    if !out.status.success() {
        return;
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !sha.is_empty() {
        println!("cargo:rustc-env=MCPHOST_GIT_SHA={sha}");
    }
}
