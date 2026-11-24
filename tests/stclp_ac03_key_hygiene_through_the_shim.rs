//! PRD-mcphost-sandbox-timeout-clamp-live-probe AC3 -- with a recording `curl`
//! shim first on PATH and a fixture key, the script's own `rpc` path drives
//! signup, quickstart, publish, offboard and whoami; the fixture key is in no
//! recorded argv, in neither captured stdout nor stderr, and only in the
//! recorded stdin of the keyed calls. The script's `--selftest` reports ok.

use std::path::{Path, PathBuf};
use std::process::Command;

const KEY: &str = "stclp-fixture-key-ac03-5e1d";

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
    let dir = std::env::temp_dir().join(format!("stclp-ac03-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("bin")).expect("mkdir");
    let curl = dir.join("bin/curl");
    std::fs::write(&curl, SHIM).expect("write shim");
    std::fs::set_permissions(&curl, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("chmod");
    let warn = r#"{"code":"timeout_clamped","field":"timeout_s","requested":45,"ceiling":30,"ceiling_field":"sandbox_cpu_s_max","effective":30,"plan":"free","message":"m"}"#;
    let starter = r#"{"starter_tool":{"publish_call":{"arguments":{"name":"starter","spec":{"source":"def run(): pass"}}}}}"#;
    let dead = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32600,"message":"tenant is disabled","data":{"error_code":"tenant_disabled","disabled_reason":"self_offboard"}}}"#;
    let replies = [
        ("signup", rpc(&format!(r#"{{"key":"{KEY}","tenant_id":"t1"}}"#))),
        ("host.quickstart", rpc(starter)),
        ("host.tool_publish", rpc(&format!(r#"{{"tested_version":1,"warnings":[{warn}]}}"#))),
        ("host.self.offboard", rpc(r#"{"offboarded":true}"#)),
        ("host.whoami", dead.to_string()),
    ];
    for (verb, body) in replies {
        std::fs::write(dir.join(format!("reply.{verb}")), body).expect("write reply");
    }
    dir
}

fn read(dir: &Path, name: &str) -> String {
    std::fs::read_to_string(dir.join(name)).unwrap_or_default()
}

#[test]
fn stclp_ac03_fixture_key_only_on_stdin_of_keyed_calls() {
    let dir = shim_dir("flow");
    let path = format!("{}:{}", dir.join("bin").display(), std::env::var("PATH").unwrap_or_default());
    let out = Command::new("bash")
        .arg("scripts/live/timeout-clamp.sh")
        .arg("http://shim.invalid")
        .env("PATH", path)
        .env("SHIM_DIR", &dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run script");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(out.status.code(), Some(0), "stdout: {stdout}\nstderr: {stderr}");
    assert_eq!(
        read(&dir, "calls").lines().collect::<Vec<_>>(),
        ["signup", "host.quickstart", "host.tool_publish", "host.self.offboard", "host.whoami"]
    );
    assert!(!stdout.contains(KEY) && !stderr.contains(KEY), "key in a capture");
    for n in 1..=5 {
        assert!(!read(&dir, &format!("call.{n}.argv")).contains(KEY), "key in argv of call {n}");
    }
    assert!(read(&dir, "call.1.stdin").is_empty(), "signup is unkeyed");
    for n in 2..=5 {
        assert_eq!(
            read(&dir, &format!("call.{n}.stdin")),
            format!("header = \"Authorization: Bearer {KEY}\"\n"),
            "call {n} stdin"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn stclp_ac03_selftest_reports_ok() {
    let out = Command::new("bash")
        .arg("scripts/live/timeout-clamp.sh")
        .arg("--selftest")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run selftest");
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).lines().any(|l| l == "selftest: ok"));
}
