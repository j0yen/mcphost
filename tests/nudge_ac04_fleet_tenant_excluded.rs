//! PRD-mcphost-second-session-nudge
//! AC4 (P0) — Given a tenant with `source_class = fleet` meeting every
//! other condition, When the sweep runs, Then it is not selected.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

const FLEET_IP: &str = "203.0.113.60";

#[tokio::test]
async fn fleet_tenant_is_never_selected_even_30h_silent() {
    let server = TestServer::start_with_signup_rate_limit_and_fleet_ips(100, FLEET_IP).await;
    let client = McpClient::new(&server.base_url);

    let raw = client
        .tools_call_with_header(
            "signup",
            json!({"name": "AC4 Fleet Tenant"}),
            ("x-forwarded-for", FLEET_IP),
        )
        .await
        .expect("fleet signup");
    let signup = extract_structured(&raw);
    let namespace = signup["tenant"].as_str().expect("tenant").to_string();
    let key = signup["key"].as_str().expect("key").to_string();

    let tenant_client = McpClient::with_bearer(&server.base_url, &key);
    tenant_client.tools_call("host.whoami", json!({})).await.expect("whoami");

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(namespace)
        .await
        .expect("find tenant")
        .expect("tenant exists");
    assert_eq!(tenant.source_class.as_deref(), Some("fleet"), "{tenant:?}");

    let backdated = mcphost::state::now_unix() - 30 * 3600;
    server
        .state
        .db
        .set_tenant_stamp_for_test(tenant.id, "first_call_unix", backdated)
        .await
        .expect("backdate first_call_unix");

    let report = mcphost::returns::sweep(&server.state).await.expect("sweep");
    assert_eq!(report.selected, 0, "a fleet tenant must never be selected: {report:?}");

    let (nudged_unix, nudge_channel, _attempts, _second_session_unix) = server
        .state
        .db
        .nudge_status(tenant.id)
        .await
        .expect("nudge_status")
        .expect("tenant exists");
    assert!(nudged_unix.is_none(), "fleet tenant row must be untouched");
    assert!(nudge_channel.is_none(), "fleet tenant row must be untouched");
}
