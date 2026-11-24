//! PRD-mcphost-registry-entry-one-source AC2 (P0) -- Given the registry stub
//! times out, When `--live` runs, Then it prints `drift=unknown` with the
//! reason and exits 2 within the harness bound below.

use std::time::Duration;

use mcphost::registry_manifest::LIVE_TIMEOUT_SECS;

use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::regone_ac01_live_compares_registry_to_endpoint::{run_live, stub_endpoint};

/// R2: "Timeouts 10 s each" (product contract, unchanged).
const PRODUCT_TIMEOUT_SECS: u64 = 10;
/// AC2 harness bound as written in the shipped PRD ("exits 2 within 11 s").
const HARNESS_BOUND_SECS: u64 = 11;

#[tokio::test]
async fn regone_ac02() {
    let endpoint = stub_endpoint("0.106.0").await;
    let registry = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(30)))
        .mount(&registry)
        .await;

    // Product contract (PRD R2): each `--live` probe times out after 10 s.
    // Pinned as a literal so raising LIVE_TIMEOUT_SECS fails here instead of
    // silently widening the test bound with it.
    const {
        assert!(
            LIVE_TIMEOUT_SECS <= PRODUCT_TIMEOUT_SECS,
            "production --live timeout exceeds the R2 contract of 10s"
        )
    };

    // Harness bound: the product timeout plus slack for process spawn and host
    // io load. It is a fixed literal, not derived from the constant, and stays
    // under 2x the timeout so a timeout applied twice (connect then request)
    // still fails, and a hang fails the test.
    let bound = Duration::from_secs(HARNESS_BOUND_SECS);
    let (code, out) = tokio::time::timeout(bound, run_live(&registry, &endpoint, &[]))
        .await
        .unwrap_or_else(|_| panic!("--live must finish within {bound:?}"));

    assert!(out.contains("drift=unknown"), "stdout: {out}");
    // The reason must be the registry probe's own timeout, at the contract's
    // 10 s (`reason=` spaces are underscores).
    assert!(
        out.contains("reason=registry_unreachable:_timed_out_after_10s"),
        "reason must name the registry timeout at 10s: {out}"
    );
    assert_eq!(code, 2);
}
