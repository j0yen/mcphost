//! PRD-mcphost-registry-listing
//! AC1 (P0) -- Given the binary, When `mcphost registry-manifest` runs,
//! Then it prints JSON with name, description, version equal to
//! `Cargo.toml`, a `remotes` entry whose URL is `MCPHOST_PUBLIC_URL` +
//! `/mcp`, and transport `streamable-http`.
//!
//! Spawns the real binary (same `env!("CARGO_BIN_EXE_mcphost")` pattern as
//! `tests/busyaudit_ac02_env_override_busy_timeout.rs`) rather than calling
//! `mcphost::registry_manifest::build_manifest` directly -- the AC's own
//! "given the binary" wording is about the CLI contract, not just the
//! underlying function.

use std::process::Command;

use serde_json::Value;

#[test]
fn prints_manifest_fields_from_cargo_toml_and_configured_endpoint() {
    let bin = env!("CARGO_BIN_EXE_mcphost");
    let output = Command::new(bin)
        .arg("registry-manifest")
        .env("MCPHOST_PUBLIC_URL", "https://example.test")
        .output()
        .expect("spawn mcphost registry-manifest");
    assert!(
        output.status.success(),
        "registry-manifest must exit 0 with no flags: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let doc: Value =
        serde_json::from_slice(&output.stdout).expect("registry-manifest stdout must be JSON");

    assert_eq!(doc["name"], mcphost::registry_manifest::full_name());
    assert_eq!(doc["description"], env!("CARGO_PKG_DESCRIPTION"));
    assert_eq!(doc["version"], env!("CARGO_PKG_VERSION"));

    let remotes = doc["remotes"].as_array().expect("remotes array");
    assert_eq!(remotes.len(), 1, "exactly one remote: {remotes:?}");
    assert_eq!(remotes[0]["type"], "streamable-http");
    assert_eq!(remotes[0]["url"], "https://example.test/mcp");
}

#[test]
fn default_public_url_is_the_production_host() {
    let bin = env!("CARGO_BIN_EXE_mcphost");
    let output = Command::new(bin)
        .arg("registry-manifest")
        .env_remove("MCPHOST_PUBLIC_URL")
        .output()
        .expect("spawn mcphost registry-manifest");
    assert!(
        output.status.success(),
        "registry-manifest must exit 0 with no flags: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let doc: Value = serde_json::from_slice(&output.stdout).expect("stdout must be JSON");
    assert_eq!(doc["remotes"][0]["url"], "https://mcphost.dev/mcp");
    assert_eq!(doc["remotes"][0]["type"], "streamable-http");
}
