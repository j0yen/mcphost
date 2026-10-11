//! AC2 — Given the same fixture and `on: ["upstream_error"]`, When called,
//! Then exactly one attempt is made and the chain fails with
//! `upstream_rate_limited`.

use crate::common;
use common::{cretr_chain_spec, cretr_server, publish};
use serde_json::json;
use std::time::Duration;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn a_429_is_not_retried_when_only_upstream_error_is_listed() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(429).set_body_string("slow down"))
        .up_to_n_times(2)
        .with_priority(1)
        .mount(&upstream)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .with_priority(2)
        .mount(&upstream)
        .await;

    let (_server, client, ns) = cretr_server(&upstream.uri(), Duration::from_secs(30)).await;
    let retry = json!({"max": 2, "backoff_ms": 10, "on": ["upstream_error"]});
    publish(&client, "pipe", "chain", cretr_chain_spec(Some(retry))).await;

    let err = client
        .tools_call(&format!("{ns}.pipe"), json!({}))
        .await
        .expect_err("a 429 outside `on` must fail the chain");
    assert_eq!(err.error_code.as_deref(), Some("upstream_rate_limited"), "{err:?}");
    assert_eq!(err.data["failed_step"], json!(2));
    assert!(err.data.get("retry_exhausted").is_none(), "{:?}", err.data);

    let requests = upstream.received_requests().await.expect("request recording on");
    assert_eq!(requests.len(), 1, "exactly one attempt reached the upstream");
}
