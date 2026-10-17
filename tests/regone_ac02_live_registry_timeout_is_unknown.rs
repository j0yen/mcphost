//! PRD-mcphost-registry-entry-one-source AC2 (P0) -- Given the registry stub
//! times out, When `--live` runs, Then it prints `drift=unknown` with the
//! reason and exits 2 within 11 s.

use std::time::Duration;

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

    // The 11 s bound is enforced by the timeout itself (a hang fails the
    // test) rather than by asserting on a measured elapsed time.
    let (code, out) = tokio::time::timeout(Duration::from_secs(11), run_live(&registry, &endpoint, &[]))
        .await
        .expect("--live must finish within 11 s");

    assert!(out.contains("drift=unknown"), "stdout: {out}");
    assert!(out.contains("reason=") && out.contains("registry"), "reason missing: {out}");
    assert_eq!(code, 2);
}
