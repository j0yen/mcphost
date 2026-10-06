//! PRD-mcphost-activation-funnel
//! AC7 (P1) — Given invite-born and organic tenants in the window, When
//! `admin.funnel {invited: true}` is read, Then only invite-born tenants
//! are counted and `k` is reported.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn invited_true_counts_only_invite_born_tenants_and_reports_k() {
    let server = TestServer::start().await;

    // Two organic (plain-signup) tenants.
    let (_organic1_ns, _organic1_key) = signup(&server.base_url, "AC7 Organic One").await;
    let (_organic2_ns, _organic2_key) = signup(&server.base_url, "AC7 Organic Two").await;

    // One inviter (also organic) and one invite-born tenant, through the
    // standing-invite path (`host.whoami`'s own `invite_url`), same
    // precedent as invite_ac11_lineage_chain_lookup_and_usage_k.rs.
    let (_inviter_ns, inviter_key) = signup(&server.base_url, "AC7 Inviter").await;
    let inviter_client = McpClient::with_bearer(&server.base_url, &inviter_key);
    let whoami =
        extract_structured(&inviter_client.tools_call("host.whoami", json!({})).await.expect("whoami"));
    let invite_url = whoami["invite_url"].as_str().expect("invite_url present").to_string();
    let invite_path = invite_url.trim_start_matches(&server.base_url).to_string();

    let invitee_session = McpClient::new(&server.base_url).with_path(&invite_path).with_session_continuity();
    invitee_session
        .tools_call("host.whoami", json!({}))
        .await
        .expect("invitee's first call (joining the standing invite) must succeed");

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    let invited_true = extract_structured(
        &admin
            .tools_call("admin.funnel", json!({"days": 7, "invited": true}))
            .await
            .expect("admin.funnel invited=true"),
    );
    assert_eq!(
        invited_true["signups"],
        json!(1),
        "only the one invite-born tenant must be counted: {invited_true}"
    );
    assert_eq!(invited_true["invited"], json!(true), "{invited_true}");
    // The inviter and the invitee both accepted/joined within this
    // window, so exactly one active inviter accepted exactly one join:
    // k = 1 / 1 = 1.0 (same shape invite_ac11's own k assertion pins).
    assert_eq!(invited_true["k"].as_f64(), Some(1.0), "{invited_true}");

    let invited_false = extract_structured(
        &admin
            .tools_call("admin.funnel", json!({"days": 7, "invited": false}))
            .await
            .expect("admin.funnel invited=false"),
    );
    assert_eq!(
        invited_false["signups"],
        json!(3),
        "the 2 organic tenants plus the organic inviter, none invite-born: {invited_false}"
    );
    assert_eq!(invited_false["invited"], json!(false), "{invited_false}");
}
