//! PRD-mcphost-registry-entry-one-source AC4 (P0) -- Given
//! `scripts/registry-submit.sh --dry-run` with no
//! `MCPHOST_REGISTRY_PRIVATE_KEY` in the environment, When it runs against
//! the stubs, Then it prints the check, validate and live lines plus the
//! publish command with the key shown as `$MCPHOST_REGISTRY_PRIVATE_KEY`,
//! exits 0, and its output contains no string longer than 20 chars from any
//! env var.

use crate::regone_ac01_live_compares_registry_to_endpoint::{stub_endpoint, stub_registry};

#[tokio::test]
async fn regone_ac04() {
    let endpoint = stub_endpoint("0.106.0").await;
    let remote = format!("{}/mcp", endpoint.uri());
    let registry = stub_registry("0.97.0", &remote).await;

    let out = tokio::process::Command::new("bash")
        .arg("scripts/registry-submit.sh")
        .arg("--dry-run")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env_remove("MCPHOST_REGISTRY_PRIVATE_KEY")
        .env("MCPHOST_BIN", env!("CARGO_BIN_EXE_mcphost"))
        .env("MCPHOST_PUBLIC_URL", endpoint.uri())
        .env("MCPHOST_REGISTRY_SEARCH_URL", format!("{}/v0.1/servers/dev.mcphost%2Fmcphost/versions/latest", registry.uri()))
        .output()
        .await
        .expect("spawn registry-submit.sh");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(out.status.code(), Some(0), "stdout: {stdout}\nstderr: {stderr}");

    let lines: Vec<&str> = stdout.lines().collect();
    let line = |p: &str| lines.iter().find(|l| l.starts_with(p)).copied().unwrap_or_default();
    assert!(line("check:").contains("up to date"), "{stdout}");
    assert!(line("validate:").contains("schema-valid"), "{stdout}");
    assert!(
        line("live:").contains("registry=0.97.0 live=0.106.0")
            && line("live:").contains("drift=yes"),
        "{stdout}"
    );
    assert!(line("publish:").contains("mcp-publisher publish"), "{stdout}");
    assert!(line("publish:").contains("$MCPHOST_REGISTRY_PRIVATE_KEY"), "{stdout}");

    // No ambient env-var value longer than 20 chars appears in the output,
    // whole or as a whitespace-separated token. (The stub URLs the script is
    // pointed at are exempt from the substring form: `remotes=<url>` must
    // print the stub's own URL by design.)
    let stub_envs = [endpoint.uri(), format!("{}/v0.1/servers/dev.mcphost%2Fmcphost/versions/latest", registry.uri())];
    for (k, v) in std::env::vars() {
        if v.len() > 20 {
            assert!(!stdout.contains(&v), "output leaks env var {k}");
        }
    }
    for tok in stdout.split_whitespace().filter(|t| t.len() > 20) {
        let leaked = std::env::vars().any(|(_, v)| v == tok) || stub_envs.iter().any(|v| v == tok);
        assert!(!leaked, "token {tok} equals an env var value");
    }
}
