//! AC7 — Given the chain of AC1, When `host.tool_test` runs, Then the step
//! report carries `retry: {max: 2, backoff_ms: 10, on:
//! ["upstream_rate_limited"]}` and no `attempts`.

use crate::common;
use common::{cretr_chain_spec, cretr_server, extract_structured, publish};
use serde_json::json;
use std::time::Duration;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn tool_test_reports_the_resolved_retry_policy_and_never_retries() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(429).set_body_string("slow down"))
        .mount(&upstream)
        .await;

    let (_server, client, _ns) = cretr_server(&upstream.uri(), Duration::from_secs(30)).await;
    let retry = json!({"max": 2, "backoff_ms": 10, "on": ["upstream_rate_limited"]});
    publish(&client, "pipe", "chain", cretr_chain_spec(Some(retry.clone()))).await;

    let result = client
        .tools_call("host.tool_test", json!({"name": "pipe", "args": {}}))
        .await
        .expect("host.tool_test succeeds on a dry run");
    let structured = extract_structured(&result);
    assert_eq!(structured["chain_dry_run"], json!(true), "{structured}");
    let steps = structured["steps"].as_array().expect("steps report");
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[1]["retry"], retry, "{structured}");
    assert!(steps[1].get("attempts").is_none(), "{structured}");
    assert!(steps[0].get("retry").is_none(), "step 1 declared no policy: {structured}");

    let requests = upstream.received_requests().await.expect("request recording on");
    assert!(requests.len() <= 1, "a dry run never retries, saw {} requests", requests.len());
}
