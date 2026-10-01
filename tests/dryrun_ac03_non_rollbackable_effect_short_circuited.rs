//! PRD-mcphost-dry-run-side-effects
//! AC3 (adapted) — Given a webhook trigger whose tool posts to a channel,
//! When `host.trigger.test` fires it, Then `dry_run.writes` contains
//! `{store: "channel", op: "post", ...}`, `delivered: false`, and nothing
//! real is sent.
//!
//! The literal scenario needs a python-sandbox channel/msg sidecar bridge
//! that does not exist yet in this tree -- `mcphost.channel`/`mcphost.msg`
//! are not registered sandbox modules (grep `_mcphost_types.ModuleType`
//! in `src/kinds/python.rs` finds only `state`/`table`/`docs`/`lineage`),
//! and the PRD's own Technical considerations names the missing half:
//! "PRD-mcphost-sandbox-channel-msg-bridge: that PRD's requirement 4
//! implements the short-circuit for its two bridges if this PRD has not
//! landed" -- i.e. the channel/msg short-circuit is that OTHER, not-yet-
//! landed PRD's job. What IS reachable today, and what this PRD's
//! requirement 4 both specifies and this test proves, is the general
//! case every store class this PRD's `dry_run.writes` enumerates
//! (`"channel" | "msg" | "trigger" | "http"`) falls under: an effect that
//! cannot be rolled back after the fact is short-circuited BEFORE it
//! happens, not undone afterward. `http` is the one such store class this
//! tree can actually dispatch to without a sandbox at all (an `http`-kind
//! tool, `src/kinds/http.rs`), so this test exercises `host.trigger.test`
//! (an `event` trigger, same entry point AC3 names) firing an `http`-kind
//! tool, and proves the outbound request never reaches the upstream.

use crate::common;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn trigger_test_http_tool_short_circuits_and_reports_dry_run() {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"posted": true})))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Dryrun AC3 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "method": "POST",
        "url": format!("{}/v1/channel-post", upstream.uri()),
        "args_schema": {"type": "object"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "notifier", "kind": "http", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let trigger = extract_structured(
        &client
            .tools_call(
                "host.trigger.set",
                json!({
                    "tool": "notifier",
                    "kind": "event",
                    "verify": {"scheme": "none", "allow_unverified": true},
                }),
            )
            .await
            .expect("trigger.set ok"),
    );
    let trigger_id = trigger["id"].as_str().expect("trigger id").to_string();

    let tested = client
        .tools_call("host.trigger.test", json!({"id": trigger_id, "body": {}}))
        .await
        .expect("trigger.test ok");
    let structured = extract_structured(&tested);
    assert_eq!(structured["test"], json!(true), "{structured}");
    let run_id = structured["run_id"].as_str().expect("run_id").to_string();

    // Wait for the executor to pick up and finish this one queued run.
    let mut run = None;
    for _ in 0..100 {
        let got = extract_structured(
            &client
                .tools_call("host.runs.get", json!({"run_id": run_id}))
                .await
                .expect("host.runs.get ok"),
        );
        let status = got["status"].as_str().unwrap_or("");
        if status == "done" || status == "error" {
            run = Some(got);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let run = run.unwrap_or_else(|| panic!("run {run_id} never finished"));
    assert_eq!(run["status"], json!("done"), "{run}");

    let result = &run["result"];
    let dry_run = &result["dry_run"];
    assert_eq!(dry_run["delivered"], json!(false), "{result}");
    assert_eq!(dry_run["rolled_back"], json!(true), "{result}");
    let writes = dry_run["writes"].as_array().unwrap_or_else(|| panic!("writes array: {result}"));
    assert_eq!(writes.len(), 1, "{result}");
    assert_eq!(writes[0]["store"], json!("http"));

    let received = upstream.received_requests().await.expect("mock recorded");
    assert!(
        received.is_empty(),
        "a dry-run trigger must never actually hit the upstream: {}",
        received.len()
    );
}
