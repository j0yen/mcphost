//! PRD-mcphost-registry-entry-one-source AC7 (P1) -- Given `--live --json`,
//! When it runs, Then the output parses and carries `registry_version`,
//! `live_version`, `drift`, `checked_at`.

use crate::regone_ac01_live_compares_registry_to_endpoint::{run_live, stub_endpoint, stub_registry};

#[tokio::test]
async fn regone_ac07() {
    let endpoint = stub_endpoint("0.106.0").await;
    let registry = stub_registry("0.97.0", &format!("{}/mcp", endpoint.uri())).await;
    let (code, out) = run_live(&registry, &endpoint, &["--json"]).await;
    assert_eq!(code, 3, "{out}");
    let v: serde_json::Value = serde_json::from_str(out.trim()).expect("--live --json parses");
    assert_eq!(v["registry_version"], "0.97.0");
    assert_eq!(v["live_version"], "0.106.0");
    assert_eq!(v["drift"], "yes");
    assert!(v["checked_at"].as_str().is_some_and(|s| s.contains('T')), "{v}");
    assert!(v.get("remotes").is_some());
}
