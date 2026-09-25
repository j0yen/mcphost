//! PRD-mcphost-abuse-guard-ban-list
//! AC7 (P0) — Given `admin.ban.remove {id}`, When the subject calls, Then
//! it succeeds and `admin_audit` holds both the add and the remove
//! entries with the operator identity.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn ban_remove_unbans_the_subject_and_both_actions_are_audited() {
    let server = TestServer::start().await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let anon = McpClient::new(&server.base_url);

    let add = admin
        .tools_call(
            "admin.ban.add",
            json!({"subject_kind": "addr", "subject": "203.0.113.40", "ttl": "24h", "reason": "flood"}),
        )
        .await
        .expect("admin.ban.add");
    let ban_id = extract_structured(&add)["id"].as_i64().expect("ban id");

    let refused = anon
        .tools_call_with_header(
            "signup",
            json!({"name": "Still Banned"}),
            ("x-forwarded-for", "203.0.113.40"),
        )
        .await;
    assert!(refused.is_err(), "must still be banned before removal");

    admin
        .tools_call("admin.ban.remove", json!({"id": ban_id}))
        .await
        .expect("admin.ban.remove");

    anon.tools_call_with_header(
        "signup",
        json!({"name": "Unbanned Now"}),
        ("x-forwarded-for", "203.0.113.40"),
    )
    .await
    .unwrap_or_else(|e| panic!("signup must succeed once the ban is removed: {e:?}"));

    let expected_actor = mcphost::auth::hash_key(ADMIN_KEY);
    let audit = admin
        .tools_call("admin.audit_log", json!({}))
        .await
        .expect("admin.audit_log");
    let entries = extract_structured(&audit)["entries"].as_array().cloned().unwrap_or_default();

    let add_entry = entries
        .iter()
        .find(|e| e["action"] == json!("ban_add") && e["target"] == json!("203.0.113.40"))
        .unwrap_or_else(|| panic!("no ban_add audit entry in {entries:?}"));
    assert_eq!(add_entry["actor_key_id"].as_str(), Some(expected_actor.as_str()));

    let remove_entry = entries
        .iter()
        .find(|e| e["action"] == json!("ban_remove") && e["target"] == json!(ban_id.to_string()))
        .unwrap_or_else(|| panic!("no ban_remove audit entry in {entries:?}"));
    assert_eq!(remove_entry["actor_key_id"].as_str(), Some(expected_actor.as_str()));
}
