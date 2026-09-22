//! PRD-mcphost-uptime-probes
//! AC9 (P2) — Given `down_since` flips for a target, When `probe`
//! finishes, Then one inbox message exists for the owner.

use crate::common;
use crate::uptime_probes;

use common::{McpClient, TempDataDir, TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn probe_sends_one_inbox_message_when_a_target_first_goes_down() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }

    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&upstream)
        .await;
    let url_down = format!("{}/down", upstream.uri());

    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC9 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    uptime_probes::create_tables(&client).await;
    client
        .tools_call(
            "host.state.insert",
            json!({"table": "targets", "rows": [{"url": url_down, "added_at": 1.0}]}),
        )
        .await
        .expect("insert target");

    // The hand-off's own self-call: `probe` authenticates its own
    // `host.msg.send` notification with this tenant's own key, published
    // as the `self_key` secret (composition can't reach a `host.*`
    // control-plane tool -- see `probe.py`'s `_notify_owner` doc comment).
    client
        .tools_call("host.secret_set", json!({"name": "self_key", "value": key}))
        .await
        .expect("secret_set self_key");

    let probe_source = uptime_probes::probe_source();
    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "probe",
                "kind": "python",
                "spec": {
                    "source": probe_source,
                    "network": "public",
                    "secrets": ["self_key"],
                    "env": {"ENDPOINT_URL": format!("{}/mcp", server.base_url), "OWNER_NAMESPACE": ns},
                },
            }),
        )
        .await
        .expect("publish probe");

    poll_until_ready(&client, &format!("{ns}.probe"), json!({}), Duration::from_secs(20))
        .await
        .unwrap_or_else(|e| panic!("probe call must succeed: {} {}", e.code, e.message));

    let inbox = extract_structured(
        &client.tools_call("host.msg.inbox", json!({})).await.expect("inbox"),
    );
    let messages = inbox["messages"].as_array().expect("messages array");
    assert_eq!(messages.len(), 1, "{inbox:?}");
    assert_eq!(messages[0]["from_address"], json!(ns), "{inbox:?}");
    let body = messages[0]["body"].as_str().expect("body is a string");
    assert!(body.contains(&url_down), "notification body must name the target: {body}");
    assert!(body.contains("down"), "notification body must say it's down: {body}");
}
