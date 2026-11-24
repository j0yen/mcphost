//! PRD-mcphost-sandbox-timeout-clamp-live-probe AC7 -- a publish reply with no
//! `warnings` key makes stdout hold `status: error reason: publish
//! no_warning` (never `status: ok`) and end `offboarded: tenant_disabled`; rc
//! is 1 and the shim's recorded calls end `host.self.offboard`, `host.whoami`
//! (the EXIT trap ran on the error path).

use std::path::PathBuf;
use std::process::Command;

const KEY: &str = "stclp-fixture-key-ac07-3c48";

const SHIM: &str = r#"#!/usr/bin/env bash
n=$(( $(cat "$SHIM_DIR/count" 2>/dev/null || echo 0) + 1 ))
echo "$n" >"$SHIM_DIR/count"
printf '%s\n' "$@" >"$SHIM_DIR/call.$n.argv"
cat >"$SHIM_DIR/call.$n.stdin"
name=""
for a in "$@"; do case "$a" in [Mm]cp-[Nn]ame:*) name="${a#*: }" ;; esac; done
echo "$name" >>"$SHIM_DIR/calls"
cat "$SHIM_DIR/reply.$name"
"#;

fn rpc(sc: &str) -> String {
    format!(r#"{{"jsonrpc":"2.0","id":1,"result":{{"structuredContent":{sc}}}}}"#)
}

/// A scratch dir holding `bin/curl` (the shim) and one `reply.<verb>` per verb.
fn shim_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("stclp-ac07-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("bin")).expect("mkdir");
    let curl = dir.join("bin/curl");
    std::fs::write(&curl, SHIM).expect("write shim");
    std::fs::set_permissions(&curl, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("chmod");
    let starter = r#"{"starter_tool":{"publish_call":{"arguments":{"name":"starter","spec":{"source":"def run(): pass"}}}}}"#;
    let dead = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32600,"message":"tenant is disabled","data":{"error_code":"tenant_disabled","disabled_reason":"self_offboard"}}}"#;
    let replies = [
        ("signup", rpc(&format!(r#"{{"key":"{KEY}","tenant_id":"t1"}}"#))),
        ("host.quickstart", rpc(starter)),
        ("host.tool_publish", rpc(r#"{"tested_version":1,"tool_lint":[]}"#)),
        ("host.self.offboard", rpc(r#"{"offboarded":true}"#)),
        ("host.whoami", dead.to_string()),
    ];
    for (verb, body) in replies {
        std::fs::write(dir.join(format!("reply.{verb}")), body).expect("write reply");
    }
    dir
}

#[test]
fn stclp_ac07_no_warnings_key_is_an_error_and_the_trap_still_offboards() {
    let dir = shim_dir("nowarn");
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
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "stdout: {stdout}");
    assert!(!stdout.contains("status: ok"), "stdout: {stdout}");
    assert_eq!(stdout, "status: error reason: publish no_warning\noffboarded: tenant_disabled\n");
    let calls: Vec<&str> = calls.lines().collect();
    assert_eq!(&calls[calls.len() - 2..], ["host.self.offboard", "host.whoami"], "calls: {calls:?}");
}
