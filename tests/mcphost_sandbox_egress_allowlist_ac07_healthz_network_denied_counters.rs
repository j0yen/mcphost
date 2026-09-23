//! PRD-mcphost-sandbox-egress-allowlist
//! AC7 (P0) — Given the denials from AC1, AC3, and AC6, When admin healthz
//! is read, Then `network_denied.publish_plan >= 1`, `run_no_proxy >= 1`,
//! `run_plan >= 1`.

use crate::common;
use common::{ADMIN_KEY, McpClient, TempDataDir, TestServer, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

use crate::egress_proxy_lock;

#[tokio::test]
async fn healthz_reports_every_network_denial_reason() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    // See egress_proxy_lock's doc comment: serializes this test against
    // the other files in this PRD that also mutate the process-wide
    // MCPHOST_EGRESS_PROXY env var.
    let _guard = egress_proxy_lock::guard().await;
    // SAFETY: held across this whole test body via the async guard above,
    // so no other test in this binary observes a torn env var.
    unsafe {
        std::env::remove_var("MCPHOST_EGRESS_PROXY");
    }

    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    // AC1-shaped denial: a free tenant's publish -> `publish_plan`.
    let (_free_ns, free_key) = signup(&server.base_url, "AC7 Free Tenant").await;
    let free_client = McpClient::with_bearer(&server.base_url, &free_key);
    free_client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "downloader",
                "kind": "python",
                "spec": {
                    "source": "def main(args):\n    return {\"ok\": True}\n",
                    "args_schema": {"type": "object"},
                    "network": "public",
                },
            }),
        )
        .await
        .expect_err("free publish of network: public must be refused");

    // AC3-shaped denial: a pro tenant with no proxy configured -> `run_no_proxy`.
    let (pro_ns, pro_key) = signup(&server.base_url, "AC7 Pro Tenant").await;
    let pro_client = McpClient::with_bearer(&server.base_url, &pro_key);
    admin
        .tools_call("admin.plan_set", json!({"tenant": &pro_ns, "plan": "pro", "reason": "AC7 setup"}))
        .await
        .expect("admin.plan_set to pro");
    pro_client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "egress_tool",
                "kind": "python",
                "spec": {
                    "source": "def main(args):\n    return {\"ok\": True}\n",
                    "args_schema": {"type": "object"},
                    "network": "egress",
                },
            }),
        )
        .await
        .expect("pro publish of network: egress must succeed");
    pro_client
        .tools_call("host.tool_call", json!({"name": "egress_tool", "args": {}}))
        .await
        .expect_err("no proxy configured must refuse the call");

    // AC6-shaped denial: that same tool's owner drops back to free ->
    // `run_plan`. `admin.plan_set`'s billing-ledger event id embeds the
    // wall-clock second (src/admin.rs); sleeping past a second boundary
    // avoids colliding with the `plan_set` call just above for this same
    // tenant (`UNIQUE constraint failed: billing_events.event_id`, a
    // pre-existing admin.rs quirk unrelated to this PRD).
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    admin
        .tools_call("admin.plan_set", json!({"tenant": &pro_ns, "plan": "free", "reason": "AC7 downgrade"}))
        .await
        .expect("admin.plan_set to free");
    pro_client
        .tools_call("host.tool_call", json!({"name": "egress_tool", "args": {}}))
        .await
        .expect_err("a now-free tenant's existing egress tool must be refused");

    let resp = reqwest::Client::new()
        .get(format!("{}/healthz", server.base_url))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .expect("GET /healthz");
    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.expect("healthz json");

    let publish_plan = body["network_denied"]["publish_plan"]["all_time"].as_i64().unwrap_or(0);
    let run_no_proxy = body["network_denied"]["run_no_proxy"]["all_time"].as_i64().unwrap_or(0);
    let run_plan = body["network_denied"]["run_plan"]["all_time"].as_i64().unwrap_or(0);
    assert!(publish_plan >= 1, "{body:?}");
    assert!(run_no_proxy >= 1, "{body:?}");
    assert!(run_plan >= 1, "{body:?}");

    let publish_plan_24h = body["network_denied"]["publish_plan"]["24h"].as_i64().unwrap_or(0);
    let run_no_proxy_24h = body["network_denied"]["run_no_proxy"]["24h"].as_i64().unwrap_or(0);
    let run_plan_24h = body["network_denied"]["run_plan"]["24h"].as_i64().unwrap_or(0);
    assert!(publish_plan_24h >= 1, "{body:?}");
    assert!(run_no_proxy_24h >= 1, "{body:?}");
    assert!(run_plan_24h >= 1, "{body:?}");
}
