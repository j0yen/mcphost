//! PRD-mcphost-empty-call-refusal-stated-in-schema
//! contract: reads scripts/live/empty-call.sh
//! AC6 (P0) -- Given the PR head, When `scripts/live/empty-call.sh <verb>`
//! runs, Then it reads the operator key into one variable (never printed),
//! posts `{}` with the content-type, accept and mcp-protocol-version headers
//! and the `_meta` fields, prints `error.data` (or `result`) as one JSON
//! object, and `--selftest` passes offline.

use std::path::Path;
use std::process::Command;

const SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/live/empty-call.sh");

#[test]
fn ecs_ac06_selftest_passes_offline() {
    let out = Command::new("bash").arg(SCRIPT).arg("--selftest").output().expect("run empty-call.sh");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "selftest failed: {stdout} {}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("selftest: ok"), "{stdout}");
}

#[test]
fn ecs_ac06_script_posts_the_required_headers_and_never_prints_the_key() {
    assert!(Path::new(SCRIPT).exists());
    let text = std::fs::read_to_string(SCRIPT).expect("script");
    for needle in [
        "content-type: application/json",
        "accept: application/json, text/event-stream",
        "mcp-protocol-version:",
        "io.modelcontextprotocol/protocolVersion",
        "io.modelcontextprotocol/clientCapabilities",
        "ssh \"$SSH_HOST\"",
    ] {
        assert!(text.contains(needle), "empty-call.sh lacks `{needle}`");
    }
    // The key travels in KEY and reaches curl on stdin, never in argv or output.
    assert_eq!(text.matches("KEY=$(").count(), 1, "the key is read into exactly one variable");
    assert!(!text.contains("echo \"$KEY\"") && !text.contains("-H \"Authorization"), "the key must not be echoed or put on a command line");
}

#[test]
fn ecs_ac06_a_missing_verb_is_a_usage_error() {
    let out = Command::new("bash").arg(SCRIPT).output().expect("run empty-call.sh");
    assert_eq!(out.status.code(), Some(2));
}
