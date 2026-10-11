//! AC1 — Given a chain whose step 2 declares `retry: {max: 2, backoff_ms:
//! 10, on: ["upstream_rate_limited"]}` and a fixture upstream answering 429
//! twice then 200, When called, Then the chain ends `done`, the step trace
//! `attempts` has 3 entries with the first two `error_code:
//! upstream_rate_limited`, and the child run's `attempt` is 3.

use crate::common;
use common::{cretr_chain_spec, cretr_children, cretr_server, extract_structured, publish};
use serde_json::json;
use std::time::Duration;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn two_429s_then_200_is_a_three_attempt_success() {
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
    let retry = json!({"max": 2, "backoff_ms": 10, "on": ["upstream_rate_limited"]});
    publish(&client, "pipe", "chain", cretr_chain_spec(Some(retry))).await;

    let out = client
        .tools_call(&format!("{ns}.pipe"), json!({}))
        .await
        .expect("the third attempt succeeds, so the chain succeeds");
    let structured = extract_structured(&out);
    assert!(structured["failed_steps"].as_array().unwrap().is_empty(), "{structured}");
    let step2 = &structured["steps"][1];
    assert_eq!(step2["status"], json!("done"), "{structured}");
    let attempts = step2["attempts"].as_array().expect("attempts array on step 2");
    assert_eq!(attempts.len(), 3, "{step2}");
    assert_eq!(attempts[0]["error_code"], json!("upstream_rate_limited"));
    assert_eq!(attempts[1]["error_code"], json!("upstream_rate_limited"));
    assert!(attempts[2].get("error_code").is_none(), "{step2}");
    assert!(structured["steps"][0].get("attempts").is_none(), "step 1 declared no retry");

    let children = cretr_children(&client).await;
    assert_eq!(children.len(), 2, "one child row per step, not per attempt: {children:?}");
    assert_eq!(children[1]["attempt"], json!(3), "{children:?}");
    assert_eq!(children[0]["attempt"], json!(1));
}
