//! PRD-mcphost-uptime-probes
//! AC2 — Given two targets and `host.trigger.fire` twice, When the `up` and
//! `latency_ms` series are read with `host.ts.get`, Then each target has exactly
//! 2 points in each (PRD-mcphost-timeseries-buckets R6 moved the example from a
//! `checks` table to series).

use crate::common;
use crate::uptime_probes;

use common::{McpClient, TempDataDir, TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn checks_has_two_rows_per_target_after_two_fires() {
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
    Mock::given(method("GET"))
        .and(path("/b"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&upstream)
        .await;
    let url_a = format!("{}/a", upstream.uri());
    let url_b = format!("{}/b", upstream.uri());

    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let _egress_guard = uptime_probes::grant_egress(&server, &ns).await;

    uptime_probes::set_targets(&client, &[url_a.clone(), url_b.clone()]).await;

    uptime_probes::publish_probe(&client).await;
    let trigger_id = uptime_probes::set_probe_schedule(&client).await;

    for _ in 0..2 {
        let done = uptime_probes::fire_and_wait(&client, &trigger_id).await;
        assert_eq!(done["status"], json!("done"), "{done:?}");
    }

    for url in [&url_a, &url_b] {
        for series in ["up", "latency_ms"] {
            assert_eq!(uptime_probes::series_points(&client, series, url).await, 2, "{series} points of {url}");
        }
        // Both fires reached a 200: the `up` series holds two 1s in one bucket.
        let up = extract_structured(
            &client
                .tools_call("host.ts.get", json!({"series": "up", "bucket": "1d", "from": "-24h", "tags": {"url": url}}))
                .await
                .expect("ts.get up"),
        );
        assert_eq!(up["buckets"][0]["min"], json!(1.0), "{url}: {up}");
    }
}
