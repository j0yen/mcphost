//! PRD-mcphost-uptime-probes
//! AC4 — Given a `checks` row older than 24 h inserted by the proof, When
//! `probe` runs, Then the row is gone.

use crate::common;
use crate::uptime_probes;

use common::{McpClient, TempDataDir, TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn probe_deletes_checks_rows_older_than_24h() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }

    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/a"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&upstream)
        .await;
    let url = format!("{}/a", upstream.uri());

    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let _egress_guard = uptime_probes::grant_egress(&server, &ns).await;

    uptime_probes::create_tables(&client).await;
    client
        .tools_call(
            "host.state.insert",
            json!({"table": "targets", "rows": [{"url": url, "added_at": 1.0}]}),
        )
        .await
        .expect("insert target");

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock")
        .as_secs_f64();
    let stale_at = now - (25 * 60 * 60) as f64; // 25h old, past the 24h cutoff.
    client
        .tools_call(
            "host.state.insert",
            json!({"table": "checks", "rows": [
                {"url": url, "code": 200, "ms": 5.0, "at": stale_at},
            ]}),
        )
        .await
        .expect("insert stale check row");

    uptime_probes::publish_probe(&client).await;
    poll_until_ready(&client, &format!("{ns}.probe"), json!({}), Duration::from_secs(20))
        .await
        .unwrap_or_else(|e| panic!("probe call must succeed: {} {}", e.code, e.message));

    let result = extract_structured(
        &client
            .tools_call("host.state.query", json!({"table": "checks", "where": format!("url = '{url}'")}))
            .await
            .expect("query checks"),
    );
    let rows = result["rows"].as_array().expect("rows array");
    assert!(
        rows.iter().all(|r| r["at"].as_f64().expect("at is a number") > now - (24 * 60 * 60) as f64),
        "no checks row may be older than 24h after probe runs, got {rows:?}"
    );
    assert!(
        !rows.iter().any(|r| r["at"].as_f64() == Some(stale_at)),
        "the stale row the proof inserted must be gone, got {rows:?}"
    );
}
