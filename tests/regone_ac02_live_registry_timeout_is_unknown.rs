//! PRD-mcphost-registry-entry-one-source AC2 (P0) -- Given the registry stub
//! times out, When `--live` runs, Then it prints `drift=unknown` with the
//! reason and exits 2 within the bound below.

use std::time::Duration;

use mcphost::registry_manifest::LIVE_TIMEOUT_SECS;

use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::regone_ac01_live_compares_registry_to_endpoint::{run_live, stub_endpoint};

#[tokio::test]
async fn regone_ac02() {
    let endpoint = stub_endpoint("0.106.0").await;
    let registry = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(30)))
        .mount(&registry)
        .await;

    // The production timeout (LIVE_TIMEOUT_SECS, applied once to both
    // concurrent probes) plus 5 s of slack for process spawn and host io load.
    // The bound stays under 2x the timeout, so a timeout applied twice
    // (connect then request) still fails, and a hang fails the test.
    let bound = Duration::from_secs(LIVE_TIMEOUT_SECS + 5);
    let (code, out) = tokio::time::timeout(bound, run_live(&registry, &endpoint, &[]))
        .await
        .unwrap_or_else(|_| panic!("--live must finish within {bound:?}"));

    assert!(out.contains("drift=unknown"), "stdout: {out}");
    assert!(out.contains("reason=") && out.contains("registry"), "reason missing: {out}");
    assert_eq!(code, 2);
}
