//! PRD-mcphost-uptime-probes
//! AC5 — Given 21 targets, When `probe` runs, Then it probes 20 and
//! reports the cap in its result.

use crate::common;
use crate::uptime_probes;

use common::{McpClient, TempDataDir, TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn probe_caps_at_20_targets_and_reports_it() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }

    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&upstream)
        .await;

    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC5 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    uptime_probes::create_tables(&client).await;
    let rows: Vec<_> = (0..21)
        .map(|i| json!({"url": format!("{}/t{i}", upstream.uri()), "added_at": i as f64}))
        .collect();
    client
        .tools_call("host.state.insert", json!({"table": "targets", "rows": rows}))
        .await
        .expect("insert 21 targets");

    uptime_probes::publish_probe(&client).await;
    let result = extract_structured(
        &poll_until_ready(&client, &format!("{ns}.probe"), json!({}), Duration::from_secs(30))
            .await
            .unwrap_or_else(|e| panic!("probe call must succeed: {} {}", e.code, e.message)),
    );

    assert_eq!(result["probed"], json!(20), "{result:?}");
    assert_eq!(result["total_targets"], json!(21), "{result:?}");
    assert_eq!(result["capped"], json!(true), "{result:?}");
}
