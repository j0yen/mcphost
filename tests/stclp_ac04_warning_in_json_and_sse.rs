//! PRD-mcphost-sandbox-timeout-clamp-live-probe AC4 -- with a publish reply
//! carrying `result.warnings[0]` (`timeout_clamped`, requested 45, effective
//! 30, plan free), once as a JSON body and once as SSE `data:` lines, stdout is
//! `status: ok`, `warning: timeout_s 45 -> 30 free`, `offboarded:
//! tenant_disabled` and the exit code is 0.

use std::path::PathBuf;
use std::process::Command;

const KEY: &str = "stclp-fixture-key-ac04-9b72";

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
fn shim_dir(tag: &str, sse: bool) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("stclp-ac04-{tag}-{}", std::process::id()));
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
        let body = if sse { format!("event: message\ndata: {body}\n\n") } else { body };
        std::fs::write(dir.join(format!("reply.{verb}")), body).expect("write reply");
    }
    dir
}

fn run(sse: bool) -> (String, i32) {
    let dir = shim_dir(if sse { "sse" } else { "json" }, sse);
    let path = format!("{}:{}", dir.join("bin").display(), std::env::var("PATH").unwrap_or_default());
    let out = Command::new("bash")
        .arg("scripts/live/timeout-clamp.sh")
        .arg("http://shim.invalid")
        .env("PATH", path)
        .env("SHIM_DIR", &dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run script");
    let _ = std::fs::remove_dir_all(&dir);
    (String::from_utf8_lossy(&out.stdout).to_string(), out.status.code().unwrap_or(-1))
}

const WANT: &str = "status: ok\nwarning: timeout_s 45 -> 30 free\noffboarded: tenant_disabled\n";

#[test]
fn stclp_ac04_json_body_prints_the_warning() {
    assert_eq!(run(false), (WANT.to_string(), 0));
}

#[test]
fn stclp_ac04_sse_data_lines_print_the_warning() {
    assert_eq!(run(true), (WANT.to_string(), 0));
}
