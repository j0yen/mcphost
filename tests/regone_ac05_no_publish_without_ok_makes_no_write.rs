//! PRD-mcphost-registry-entry-one-source AC5 (P0) -- Given
//! `scripts/registry-submit.sh` without `--dry-run` and no `PUBLISH-OK`,
//! When it runs, Then it prints `would publish ...` and exits 0 having made
//! no network write (stub records zero POSTs).

use crate::regone_ac01_live_compares_registry_to_endpoint::{stub_endpoint, stub_registry};

#[tokio::test]
async fn regone_ac05() {
    let endpoint = stub_endpoint("0.106.0").await;
    let registry = stub_registry("0.97.0", &format!("{}/mcp", endpoint.uri())).await;

    let out = tokio::process::Command::new("bash")
        .arg("scripts/registry-submit.sh")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("MCPHOST_REGISTRY_PRIVATE_KEY", "test-key-not-a-secret")
        .env("MCPHOST_PUBLISH_OK", "/nonexistent/PUBLISH-OK")
        .env("MCPHOST_BIN", env!("CARGO_BIN_EXE_mcphost"))
        .env("MCPHOST_PUBLIC_URL", endpoint.uri())
        .env("MCPHOST_REGISTRY_SEARCH_URL", format!("{}/v0.1/servers/dev.mcphost%2Fmcphost/versions/latest", registry.uri()))
        .output()
        .await
        .expect("spawn registry-submit.sh");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert_eq!(out.status.code(), Some(0), "stdout: {stdout}");
    assert!(stdout.contains("would publish"), "{stdout}");
    assert!(!stdout.contains("test-key-not-a-secret"), "key echoed: {stdout}");

    for server in [&endpoint, &registry] {
        let posts = server
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .filter(|r| r.method.as_str() == "POST")
            .count();
        assert_eq!(posts, 0, "no POST may be made without PUBLISH-OK");
    }
}
