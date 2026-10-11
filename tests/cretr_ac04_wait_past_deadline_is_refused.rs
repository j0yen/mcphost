//! AC4 — Given a 429 with `retry_after_s: 7` and a chain with 3 s of
//! deadline remaining, When the retry is evaluated, Then no wait occurs and
//! the error carries `data.retry_deadline_exceeded: true` within 1 s.

use crate::common;
use common::{cretr_chain_spec, cretr_server, publish};
use serde_json::json;
use std::time::{Duration, Instant};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn retry_after_longer_than_the_deadline_stops_without_waiting() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "7").set_body_string("slow down"))
        .mount(&upstream)
        .await;

    let (_server, client, ns) = cretr_server(&upstream.uri(), Duration::from_secs(3)).await;
    let retry = json!({"max": 2, "backoff_ms": 10, "on": ["upstream_rate_limited"]});
    publish(&client, "pipe", "chain", cretr_chain_spec(Some(retry))).await;

    let started = Instant::now();
    let err = client
        .tools_call(&format!("{ns}.pipe"), json!({}))
        .await
        .expect_err("a 7 s wait does not fit a 3 s deadline");
    let elapsed = started.elapsed();
    assert!(elapsed < Duration::from_secs(1), "no wait may occur, took {elapsed:?}");
    assert_eq!(err.error_code.as_deref(), Some("upstream_rate_limited"), "{err:?}");
    assert_eq!(err.data["retry_deadline_exceeded"], json!(true), "{:?}", err.data);
    assert_eq!(err.data["retry_after_s"], json!(7));
    assert!(err.data.get("retry_exhausted").is_none(), "{:?}", err.data);

    let requests = upstream.received_requests().await.expect("request recording on");
    assert_eq!(requests.len(), 1, "no second attempt was made");
}
