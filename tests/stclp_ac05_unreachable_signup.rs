//! PRD-mcphost-sandbox-timeout-clamp-live-probe AC5 -- against `http://
//! 127.0.0.1:9` the script prints `status: error reason: signup unreachable`,
//! exits 1 within 30 s, and makes no offboard call (no key exists).

use std::process::Command;

const SHIM: &str = r#"#!/usr/bin/env bash
n=$(( $(cat "$SHIM_DIR/count" 2>/dev/null || echo 0) + 1 ))
echo "$n" >"$SHIM_DIR/count"
for a in "$@"; do case "$a" in [Mm]cp-[Nn]ame:*) echo "${a#*: }" >>"$SHIM_DIR/calls" ;; esac; done
cat >/dev/null
exit 7
"#;

#[test]
fn stclp_ac05_discard_port_is_signup_unreachable_within_30s() {
    let start = std::time::Instant::now();
    let out = Command::new("bash")
        .arg("scripts/live/timeout-clamp.sh")
        .arg("http://127.0.0.1:9")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run script");
    let wall_s = start.elapsed().as_secs_f64();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout, "status: error reason: signup unreachable\n", "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(out.status.code(), Some(1));
    assert!(wall_s < 30.0, "took {wall_s}s");
}

#[test]
fn stclp_ac05_no_offboard_call_when_no_key_exists() {
    let dir = std::env::temp_dir().join(format!("stclp-ac05-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("bin")).expect("mkdir");
    let curl = dir.join("bin/curl");
    std::fs::write(&curl, SHIM).expect("write shim");
    std::fs::set_permissions(&curl, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("chmod");
    let path = format!("{}:{}", dir.join("bin").display(), std::env::var("PATH").unwrap_or_default());
    let out = Command::new("bash")
        .arg("scripts/live/timeout-clamp.sh")
        .arg("http://shim.invalid")
        .env("PATH", path)
        .env("SHIM_DIR", &dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run script");
    let calls = std::fs::read_to_string(dir.join("calls")).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "status: error reason: signup unreachable\n");
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(calls.lines().collect::<Vec<_>>(), ["signup"], "only the signup was attempted");
}
