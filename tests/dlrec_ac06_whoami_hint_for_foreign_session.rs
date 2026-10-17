//! PRD-mcphost-delegation-recipes-and-quickstart
//! AC6 (P1) — Given a delegated key used from a connection whose session id
//! differs from the minting one, When it calls `host.whoami`, Then `hint` is
//! present and equals the recipe's prose line; Given the same session, Then no
//! `hint`.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use mcphost::delegation_recipe::{HELPER_HINT, delegated_hint, sub_agent_key};
use serde_json::json;

#[tokio::test]
async fn dlrec_ac06_hint_only_from_a_session_other_than_the_minting_one() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Recipe Hint Tenant").await;
    let lead = McpClient::with_bearer(&server.base_url, &key).with_session_continuity();
    lead.tools_call("host.whoami", json!({})).await.expect("lead whoami opens the session");

    let delegated = extract_structured(
        &lead
            .tools_call("host.key.delegate", json!({"scopes": ["tools:call"], "ttl_s": 600, "label": "scout"}))
            .await
            .expect("delegate"),
    );
    let child_key = delegated["key"].as_str().expect("child key");

    // Same session as the minting one: no hint.
    let same = McpClient::with_bearer(&server.base_url, child_key).joining_session_of(&lead);
    let me = extract_structured(&same.tools_call("host.whoami", json!({})).await.expect("same-session whoami"));
    assert_eq!(me["key"]["label"], json!("scout"), "{me}");
    assert!(me.get("hint").is_none(), "no hint on the minting session: {me}");

    // A second connection: the hint is the recipe's prose line, label filled in.
    let other = McpClient::with_bearer(&server.base_url, child_key).with_session_continuity();
    let me = extract_structured(&other.tools_call("host.whoami", json!({})).await.expect("other-session whoami"));
    assert_eq!(me["hint"], json!(delegated_hint("scout")), "{me}");
    assert_eq!(me["hint"], json!(HELPER_HINT.replace("<label>", "scout")));
    let prose_last = sub_agent_key().prose.last().cloned().expect("recipe prose");
    assert_eq!(prose_last, HELPER_HINT, "the recipe's last prose line is the hint's one source");

    // The primary key never gets a hint.
    let primary = extract_structured(&lead.tools_call("host.whoami", json!({})).await.expect("primary whoami"));
    assert!(primary.get("hint").is_none(), "{primary}");
}
