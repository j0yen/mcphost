//! PRD-mcphost-uptime-probe-recipe-green
//! AC7 -- Given a target that answers after 4 s, When the probe fires, Then that
//! row is `status: "timeout"`, the other rows are correct, and the fire
//! completes within 10 s.

use crate::uprg;

use serde_json::json;
use std::time::Duration;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn slow_target_is_timeout_others_correct_within_ten_seconds() {
    let fx = uprg::start().await;
    let slow = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(4)))
        .mount(&slow)
        .await;
    let slow_url = format!("{}/probe-target-slow", slow.uri());
    let urls = vec![fx.urls[0].clone(), slow_url.clone(), fx.urls[2].clone()];

    // A deadline, not a timing comparison: the create call (which runs the
    // first probe) must answer inside 10 s despite the 4 s target.
    let out = tokio::time::timeout(Duration::from_secs(10), fx.create(json!({"name": "status", "urls": urls})))
        .await
        .expect("create answers within 10 s")
        .expect("create");

    let rows = out["first_result"]["rows"].as_array().unwrap_or_else(|| panic!("{out}"));
    let row_of = |url: &str| rows.iter().find(|r| r["url"] == json!(url)).unwrap_or_else(|| panic!("no row for {url}: {out}"));
    assert_eq!(row_of(&slow_url)["status"], json!("timeout"), "{out}");
    assert_eq!(row_of(&slow_url)["ok"], json!(false), "{out}");
    assert_eq!(row_of(&fx.urls[0])["ok"], json!(true), "{out}");
    assert_eq!(row_of(&fx.urls[0])["status"], json!("200"), "{out}");
    assert_eq!(row_of(&fx.urls[2])["ok"], json!(false), "{out}");

    // A later fire of the probe tool itself behaves the same.
    let raw = tokio::time::timeout(
        Duration::from_secs(10),
        fx.client.tools_call("host.tool_call", json!({"name": "status_probe"})),
    )
    .await
    .expect("fire answers within 10 s")
    .expect("fire");
    let result = uprg::call_result(&raw);
    let timeouts = result["rows"].as_array().expect("rows").iter().filter(|r| r["status"] == json!("timeout")).count();
    assert_eq!(timeouts, 1, "{result}");
}
