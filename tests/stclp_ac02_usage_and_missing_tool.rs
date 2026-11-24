//! PRD-mcphost-sandbox-timeout-clamp-live-probe AC2 -- `scripts/live/
//! timeout-clamp.sh` with no argument prints `usage: timeout-clamp.sh
//! <base_url> | --selftest` on stderr and exits 2; with a `<base_url>` under a
//! `PATH` holding only `bash` it prints `timeout-clamp.sh: missing required
//! tool: curl` on stderr and exits 3. Neither path makes a request.

use std::process::Command;

/// The `bash` found on the current PATH, as an absolute path.
fn bash_path() -> std::path::PathBuf {
    let path = std::env::var_os("PATH").expect("PATH");
    std::env::split_paths(&path)
        .map(|d| d.join("bash"))
        .find(|p| p.is_file())
        .expect("bash on PATH")
}

#[test]
fn stclp_ac02_usage_on_no_argument_exit_2() {
    let out = Command::new("bash")
        .arg("scripts/live/timeout-clamp.sh")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run script");
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(String::from_utf8_lossy(&out.stderr).trim_end(), "usage: timeout-clamp.sh <base_url> | --selftest");
    assert!(out.stdout.is_empty(), "stdout: {}", String::from_utf8_lossy(&out.stdout));
}

#[test]
fn stclp_ac02_missing_curl_exit_3_without_a_request() {
    let dir = std::env::temp_dir().join(format!("stclp-ac02-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    let bash = bash_path();
    std::os::unix::fs::symlink(&bash, dir.join("bash")).expect("symlink bash");
    let out = Command::new(&bash)
        .arg("scripts/live/timeout-clamp.sh")
        .arg("http://127.0.0.1:9")
        .env("PATH", &dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run script");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(String::from_utf8_lossy(&out.stderr).trim_end(), "timeout-clamp.sh: missing required tool: curl");
    assert!(out.stdout.is_empty(), "no status line: nothing was requested");
}
