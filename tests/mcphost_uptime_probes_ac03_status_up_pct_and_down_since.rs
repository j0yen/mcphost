//! PRD-mcphost-uptime-probes
//! AC3 — Given one target returning 503, When `status` is called, Then
//! that target has `up_pct_24h` 0 and a `down_since` equal to its first
//! check; the healthy target has 100 and no `down_since`.

use crate::common;
use crate::uptime_probes;

use common::{McpClient, TempDataDir, TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn status_reports_up_pct_and_down_since() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }

    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/healthy"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&upstream)
        .await;
    Mock::given(method("GET"))
        .and(path("/down"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&upstream)
        .await;
    let url_healthy = format!("{}/healthy", upstream.uri());
    let url_down = format!("{}/down", upstream.uri());

    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC3 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let _egress_guard = uptime_probes::grant_egress(&server, &ns).await;

    uptime_probes::create_tables(&client).await;
    client
        .tools_call(
            "host.state.insert",
            json!({"table": "targets", "rows": [
                {"url": url_healthy, "added_at": 1.0},
                {"url": url_down, "added_at": 2.0},
            ]}),
        )
        .await
        .expect("insert targets");

    uptime_probes::publish_probe(&client).await;
    uptime_probes::publish_status(&client).await;

    poll_until_ready(&client, &format!("{ns}.probe"), json!({}), Duration::from_secs(20))
        .await
        .unwrap_or_else(|e| panic!("probe call must succeed: {} {}", e.code, e.message));

    let status_result = extract_structured(
        &poll_until_ready(&client, &format!("{ns}.status"), json!({}), Duration::from_secs(20))
            .await
            .unwrap_or_else(|e| panic!("status call must succeed: {} {}", e.code, e.message)),
    );
    let targets = status_result["targets"].as_array().expect("targets array");

    let healthy_entry = targets
        .iter()
        .find(|t| t["url"] == json!(url_healthy))
        .unwrap_or_else(|| panic!("no entry for the healthy target: {targets:?}"));
    assert_eq!(healthy_entry["up_pct_24h"], json!(100), "{healthy_entry:?}");
    assert!(
        healthy_entry.get("down_since").is_none() || healthy_entry["down_since"].is_null(),
        "healthy target must not have down_since: {healthy_entry:?}"
    );

    let down_entry = targets
        .iter()
        .find(|t| t["url"] == json!(url_down))
        .unwrap_or_else(|| panic!("no entry for the down target: {targets:?}"));
    assert_eq!(down_entry["up_pct_24h"], json!(0), "{down_entry:?}");

    let checks_down = extract_structured(
        &client
            .tools_call(
                "host.state.query",
                json!({"table": "checks", "where": format!("url = '{url_down}'"), "order_by": "at asc"}),
            )
            .await
            .expect("query down checks"),
    );
    let first_at = checks_down["rows"][0]["at"].clone();
    assert_eq!(down_entry["down_since"], first_at, "{down_entry:?}");
}
