//! PRD-mcphost-sandbox-egress-allowlist
//! AC6 (P0) — Given a pre-existing tool row with `network: "public"` owned
//! by a tenant now on Free, When called, Then `plan_required` with the
//! republish hint; When the same tenant upgrades to Pro with the proxy
//! set, Then the call succeeds.

use crate::common;
use common::{ADMIN_KEY, McpClient, TempDataDir, TestServer, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

use crate::egress_proxy_lock;

/// `admin.plan_set`'s billing-ledger event id is `admin.plan_set:<ns>:
/// <unix_seconds>` (src/admin.rs) -- a second `plan_set` for the same
/// tenant inside the same wall-clock second collides on that id
/// (`UNIQUE constraint failed: billing_events.event_id`), a pre-existing
/// admin.rs quirk unrelated to this PRD. Sleeping past a second boundary
/// between this test's repeated `plan_set` calls for one tenant sidesteps
/// it without touching admin.rs.
async fn past_the_next_second() {
    tokio::time::sleep(Duration::from_millis(1100)).await;
}

#[tokio::test]
async fn existing_public_tool_fails_on_downgrade_then_succeeds_after_upgrade() {
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
        std::env::set_var("MCPHOST_EGRESS_PROXY", "http://127.0.0.1:1");
    }

    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC6 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    // Publish while Pro (this PRD's AC1/AC2 gate is a publish-time-only
    // check) so the stored row carries `network: "public"`.
    admin
        .tools_call("admin.plan_set", json!({"tenant": &ns, "plan": "pro", "reason": "AC6 setup"}))
        .await
        .expect("admin.plan_set to pro");
    let spec = json!({
        "source": "def main(args):\n    return {\"ok\": True}\n",
        "args_schema": {"type": "object"},
        "network": "public",
    });
    client
        .tools_call("host.tool_publish", json!({"name": "legacy", "kind": "python", "spec": spec}))
        .await
        .expect("pro tenant publish of network: public must succeed");

    // Now the tenant drops to Free -- the existing row is untouched, but a
    // call must fail with `plan_required`, not a silent downgrade to no
    // network (requirement 3).
    past_the_next_second().await;
    admin
        .tools_call("admin.plan_set", json!({"tenant": &ns, "plan": "free", "reason": "AC6 downgrade"}))
        .await
        .expect("admin.plan_set to free");

    let err = client
        .tools_call("host.tool_call", json!({"name": "legacy", "args": {}}))
        .await
        .expect_err("a now-free tenant's existing public tool must be refused");
    assert_eq!(err.error_code.as_deref(), Some("plan_required"));
    assert_eq!(err.data["plan"], json!("pro"));
    assert_eq!(err.data["field"], json!("network"));
    assert!(
        err.message.contains("network: \"none\""),
        "the error must name the republish fix: {}",
        err.message
    );

    // Upgrading back to Pro (with the proxy already configured above) must
    // let the exact same call succeed again, no republish needed.
    past_the_next_second().await;
    admin
        .tools_call("admin.plan_set", json!({"tenant": &ns, "plan": "pro", "reason": "AC6 upgrade"}))
        .await
        .expect("admin.plan_set back to pro");

    client
        .tools_call("host.tool_call", json!({"name": "legacy", "args": {}}))
        .await
        .unwrap_or_else(|e| panic!("call must succeed once pro + proxy again: {} {}", e.code, e.message));
}
