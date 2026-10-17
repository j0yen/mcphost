//! PRD-mcphost-example-recipes-conform-to-policy
//! AC2 — Given a free tenant, When uptime-probes' proof publishes `probe`,
//! Then the spec carries `network: "none"`, publish succeeds, and the recipe
//! text names `network: "public"` as `plan: pro`.

use crate::common;
use common::{TempDataDir, TestServer, python_kind_registry};
use tokio::process::Command;

const MANIFEST: &str = include_str!("../examples/uptime-probes/example.toml");
const PROOF: &str = include_str!("../examples/uptime-probes/proof.sh");

#[tokio::test]
async fn ercp_ac02() {
    // The recipe text: manifest publishes `none`, and both files name the
    // `public` variant as `plan: pro`.
    let tools: toml::Value = toml::from_str(MANIFEST).expect("example.toml parses");
    let probe = tools["tools"]
        .as_array()
        .and_then(|a| a.iter().find(|t| t["name"].as_str() == Some("probe")))
        .expect("probe tool in example.toml");
    assert_eq!(probe["network"].as_str(), Some("none"), "manifest must publish network none: {probe}");
    for (what, text) in [("example.toml", MANIFEST), ("proof.sh", PROOF)] {
        assert!(text.contains("plan: pro"), "{what} must name the pro plan for the public variant");
        assert!(
            text.contains("network: \"public\"") || text.contains("`public`"),
            "{what} must name network public"
        );
    }
    assert!(MANIFEST.contains("network: \"public\""), "example.toml must name `network: \"public\"`");

    if mcphost::sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", mcphost::sandbox::USERNS_SKIP_MARKER);
        return;
    }
    // The proof, on a free tenant (signup default), against a host that
    // enforces the free-plan network gate.
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/uptime-probes/proof.sh");
    let out = Command::new("bash")
        .arg(&script)
        .env("MCPHOST_URL", format!("{}/mcp", server.base_url))
        .env_remove("PROBE_NETWORK")
        .output()
        .await
        .expect("run uptime-probes proof.sh");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.lines().any(|l| l == "PROBE_NETWORK=none"), "proof must publish network none:\n{stdout}");
    assert!(stdout.lines().any(|l| l == "CHECK publish_probe: PASS"), "free-tenant publish must succeed:\n{stdout}");
    assert!(out.status.success(), "proof must pass on a free tenant:\n{stdout}");
}
