//! AC3 — Given `max: 2` and a fixture answering 429 always, When called,
//! Then 3 attempts are made and the error carries `data.attempts: 3` and
//! `data.retry_exhausted: true`.

use crate::common;
use common::{cretr_chain_spec, cretr_server, publish};
use serde_json::json;
use std::time::Duration;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn an_always_429_upstream_exhausts_three_attempts() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(429).set_body_string("slow down"))
        .mount(&upstream)
        .await;

    let (_server, client, ns) = cretr_server(&upstream.uri(), Duration::from_secs(30)).await;
    let retry = json!({"max": 2, "backoff_ms": 10, "on": ["upstream_rate_limited"]});
    publish(&client, "pipe", "chain", cretr_chain_spec(Some(retry))).await;

    let err = client
        .tools_call(&format!("{ns}.pipe"), json!({}))
        .await
        .expect_err("a permanently rate-limited upstream must fail the chain");
    assert_eq!(err.error_code.as_deref(), Some("upstream_rate_limited"), "{err:?}");
    assert_eq!(err.data["attempts"], json!(3), "{:?}", err.data);
    assert_eq!(err.data["retry_exhausted"], json!(true), "{:?}", err.data);
    assert_eq!(err.data["failed_step"], json!(2));

    let requests = upstream.received_requests().await.expect("request recording on");
    assert_eq!(requests.len(), 3, "max: 2 means 3 attempts reached the upstream");
}
