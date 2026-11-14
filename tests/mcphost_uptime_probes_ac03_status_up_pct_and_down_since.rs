//! PRD-mcphost-uptime-probes
//! AC3 — Given one target returning 503, When `status` is called, Then
//! that target has `up_pct_24h` 0 and a `down_since` at its first check; the
//! healthy target has 100 and no `down_since`. (PRD-mcphost-timeseries-buckets
//! R6: `status` answers from `host.ts.get` buckets, so `down_since` is the
//! start of the whole minute holding the first failed probe.)

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

    uptime_probes::set_targets(&client, &[url_healthy.clone(), url_down.clone()]).await;

    uptime_probes::publish_probe(&client).await;
    uptime_probes::publish_status(&client).await;

    let before = now_unix();
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

    // `down_since` is a whole-minute bucket start that holds the first failed probe.
    let down_since = down_entry["down_since"].as_i64().unwrap_or_else(|| panic!("no down_since: {down_entry:?}"));
    assert_eq!(down_since % 60, 0, "a minute bucket start: {down_entry:?}");
    assert!(down_since >= before - 60 && down_since <= now_unix(), "down_since {down_since} near the probe ({before}): {down_entry:?}");
    assert_eq!(down_entry["up"], json!(false), "{down_entry:?}");
    assert_eq!(healthy_entry["up"], json!(true), "{healthy_entry:?}");
}

fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("system clock").as_secs() as i64
}
