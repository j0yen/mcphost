//! PRD-mcphost-uptime-probes
//! AC4 — Given a point older than 24 h in the `up` and `latency_ms` series, When
//! `probe` runs, Then it is gone (PRD-mcphost-timeseries-buckets R6: retention
//! trims the series; it used to delete `checks` rows).

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

    uptime_probes::set_targets(&client, &[url.clone()]).await;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock")
        .as_secs() as i64;
    let stale_at = now - 25 * 60 * 60; // 25h old, past the 24h cutoff.
    for series in ["up", "latency_ms"] {
        client
            .tools_call("host.ts.put", json!({"series": series, "value": 5, "ts": stale_at, "tags": {"url": url}}))
            .await
            .expect("put a stale point");
    }

    uptime_probes::publish_probe(&client).await;
    poll_until_ready(&client, &format!("{ns}.probe"), json!({}), Duration::from_secs(20))
        .await
        .unwrap_or_else(|e| panic!("probe call must succeed: {} {}", e.code, e.message));

    let listed = extract_structured(&client.tools_call("host.ts.list", json!({})).await.expect("ts.list"));
    let series = listed["series"].as_array().expect("series array");
    assert_eq!(series.len(), 2, "{listed}");
    for s in series {
        let oldest = s["oldest_ts"].as_i64().expect("oldest_ts");
        assert!(oldest > now - 24 * 60 * 60, "no point may be older than 24h after probe runs, got {s}");
        assert_ne!(oldest, stale_at, "the stale point the test put must be gone: {s}");
        assert_eq!(s["points"], json!(1), "only the fresh probe point is left: {s}");
    }
}
