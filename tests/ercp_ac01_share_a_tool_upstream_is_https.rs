//! PRD-mcphost-example-recipes-conform-to-policy
//! AC1 — Given a free tenant, When share-a-tool's proof runs under the runner
//! (no `MOCK_LOCAL`), Then its upstream URL starts with `https://` and the
//! publish succeeds with no `host_not_allowed`.

use crate::common;
use common::{FixedLookup, TestServer, http_kind_registry_strict};
use std::collections::HashMap;
use tokio::process::Command;

/// Same spawn the runner makes (`bash <proof>` with `MCPHOST_URL`), against a
/// host running the production-strict http policy (https-only, no loopback).
/// Publish is syntactic, so no upstream needs to be reachable for the check
/// this AC is about; later steps of the proof may fail offline and that is
/// not asserted here.
#[tokio::test]
async fn ercp_ac01() {
    let server = TestServer::start_with_kinds(http_kind_registry_strict(FixedLookup(HashMap::new()))).await;
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/share-a-tool/proof.sh");
    let out = Command::new("bash")
        .arg(&script)
        .env("MCPHOST_URL", format!("{}/mcp", server.base_url))
        .env_remove("UPSTREAM_URL")
        .env_remove("MOCK_LOCAL")
        .output()
        .await
        .expect("run share-a-tool proof.sh");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let upstream = stdout
        .lines()
        .find_map(|l| l.strip_prefix("UPSTREAM_URL="))
        .unwrap_or_else(|| panic!("proof printed no UPSTREAM_URL= line:\n{stdout}"));
    assert!(upstream.starts_with("https://"), "upstream must be https, got {upstream}");
    assert!(
        stdout.lines().any(|l| l == "CHECK owner_tool_publish: PASS"),
        "publish against the strict host must succeed:\n{stdout}"
    );
    assert!(!stdout.contains("host_not_allowed"), "publish must not be refused:\n{stdout}");
}
