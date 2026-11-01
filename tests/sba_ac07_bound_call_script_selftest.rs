//! PRD-mcphost-schema-bound-agreement
//! contract: reads scripts/live/bound-call.sh
//! AC7 (P0) -- Given the PR head, When `scripts/live/bound-call.sh <verb>
//! '<json>'` runs, Then it calls the anonymous `signup` tool with a synthetic
//! `client_name` starting `synthorg` (key never printed), posts the call to
//! prod `/mcp` with that key, prints one JSON object tagged `kind` error or
//! result, calls `host.self_offboard` in a trap once a key exists, and
//! `--selftest` passes offline.

use std::path::Path;
use std::process::Command;

const SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/live/bound-call.sh");

#[test]
fn sba_ac07_selftest_passes_offline() {
    let out = Command::new("bash").arg(SCRIPT).arg("--selftest").output().expect("run bound-call.sh");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "selftest failed: {stdout} {}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("selftest: ok"), "{stdout}");
}

#[test]
fn sba_ac07_usage_error_exits_2() {
    let out = Command::new("bash").arg(SCRIPT).output().expect("run bound-call.sh");
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn sba_ac07_script_signs_up_synthetic_posts_offboards_and_never_prints_the_key() {
    assert!(Path::new(SCRIPT).exists());
    let text = std::fs::read_to_string(SCRIPT).expect("script");
    for needle in [
        "CLIENT_NAME=\"synthorg-",
        "synthorg-bound-call-$(date",
        "post - signup",
        "host.self_offboard",
        "trap 'finish' EXIT",
        "content-type: application/json",
        "accept: application/json, text/event-stream",
        "mcp-protocol-version:",
        "io.modelcontextprotocol/clientInfo",
        "\"$BASE/mcp\"",
        "out[\"kind\"] = \"error\"",
        "else \"result\"",
    ] {
        assert!(text.contains(needle), "bound-call.sh lacks `{needle}`");
    }
    // The key reaches curl on stdin, never in argv or output.
    assert!(text.contains("-K -"), "curl reads the Authorization header from stdin");
    assert!(!text.contains("echo \"$KEY\"") && !text.contains("-H \"Authorization") && !text.contains("-H 'Authorization"), "the key must not be echoed or put on a command line");
}
