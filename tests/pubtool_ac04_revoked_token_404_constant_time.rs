//! PRD-mcphost-public-tool-url
//! AC4 (P0) — Given a revoked token (after `host.tool_unshare`), When the
//! URL is called, Then 404 is returned with a body that names neither
//! tenant nor tool, and the median response time differs from a
//! never-issued token's by less than 20 ms over 50 requests.

use crate::common;
use common::{TestServer, extract_structured, signup};
use serde_json::json;
use std::time::Instant;

fn median_millis(mut samples: Vec<f64>) -> f64 {
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mid = samples.len() / 2;
    if samples.len().is_multiple_of(2) {
        (samples[mid - 1] + samples[mid]) / 2.0
    } else {
        samples[mid]
    }
}

async fn timed_requests(http: &reqwest::Client, url: &str, n: usize) -> Vec<f64> {
    let mut samples = Vec::with_capacity(n);
    for _ in 0..n {
        let start = Instant::now();
        let resp = http.get(url).send().await.expect("GET");
        assert_eq!(resp.status(), 404, "both a revoked and a never-issued token must 404");
        let body = resp.bytes().await.expect("read body");
        assert!(body.is_empty(), "404 body must name neither tenant nor tool (empty body): {body:?}");
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    samples
}

#[tokio::test]
async fn revoked_and_never_issued_tokens_answer_404_in_roughly_the_same_time() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC4 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "echo", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish ok");
    let shared = extract_structured(
        &client
            .tools_call("host.tool_share", json!({"name": "echo", "visibility": "url"}))
            .await
            .expect("tool_share ok"),
    );
    let url = shared["url"].as_str().expect("url field").to_string();

    client
        .tools_call("host.tool_unshare", json!({"name": "echo"}))
        .await
        .expect("tool_unshare ok");

    // A token-shaped string that was never minted at all, same path shape.
    let never_issued_url = url.replacen(
        url.split('/').nth_back(1).expect("token segment"),
        "aaaaaaaaaaaaaaaaaaaaaaaaaa",
        1,
    );
    assert_ne!(url, never_issued_url);

    let http = reqwest::Client::new();
    // Requirement 6's own 60-calls-per-client-IP-per-minute limiter (AC5)
    // shares this test's single loopback peer address across both token
    // URLs, so the total request budget here (including the one warmup
    // request below) must stay under 60.
    let _ = timed_requests(&http, &url, 1).await;
    let _ = timed_requests(&http, &never_issued_url, 1).await;

    let revoked_samples = timed_requests(&http, &url, 24).await;
    let never_issued_samples = timed_requests(&http, &never_issued_url, 24).await;

    let revoked_median = median_millis(revoked_samples);
    let never_issued_median = median_millis(never_issued_samples);
    let diff = (revoked_median - never_issued_median).abs();
    assert!(
        diff < 20.0,
        "median response time must differ by < 20ms: revoked={revoked_median}ms never_issued={never_issued_median}ms diff={diff}ms"
    );
}
