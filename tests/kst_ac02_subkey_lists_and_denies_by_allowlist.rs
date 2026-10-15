//! PRD-mcphost-key-scoped-to-tools
//! AC2 (P0) — Given that sub-key, When `tools/list` runs, Then exactly `a`,
//! `host.whoami`, `host.quickstart`, `host.tool.reach` are returned; When
//! `tools/call host.secret.list` runs, Then `permission_denied` with
//! `data.allowed_tools == ["a"]` and `data.key_label`.
//!
//! `host.tool.reach` is named by the PRD's always-listed set but is built by
//! a different PRD (PRD-mcphost-tool-reach-manifest); the listing is the
//! allowlist plus whichever of `subkeys::ALWAYS_LISTED` this build serves.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;
use std::collections::BTreeSet;

#[tokio::test]
async fn subkey_tools_list_is_allowlist_plus_always_listed_and_other_calls_are_permission_denied() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Subkey Scope Tenant").await;
    let primary = McpClient::with_bearer(&server.base_url, &key);
    for name in ["ta", "tb"] {
        primary
            .tools_call(
                "host.tool.publish",
                json!({"name": name, "kind": "echo", "spec": {"schema": {"type": "object"}}}),
            )
            .await
            .expect("publish");
    }
    let minted = extract_structured(
        &primary
            .tools_call("host.key.mint", json!({"tools": ["ta"], "label": "scoped"}))
            .await
            .expect("mint"),
    );
    let sub = minted["key"].as_str().expect("key").to_string();
    let client = McpClient::with_bearer(&server.base_url, &sub);

    let listing = client.tools_list().await.expect("tools/list under the sub-key");
    let names: BTreeSet<String> = listing["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|t| t["name"].as_str().expect("name").to_string())
        .collect();
    let primary_names: BTreeSet<String> = primary.tools_list().await.expect("primary list")["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|t| t["name"].as_str().expect("name").to_string())
        .collect();
    let mut expected: BTreeSet<String> = [format!("{ns}.ta")].into_iter().collect();
    for always in mcphost::subkeys::ALWAYS_LISTED {
        if primary_names.contains(*always) {
            expected.insert((*always).to_string());
        }
    }
    assert_eq!(names, expected, "sub-key listing must be the allowlist plus the always-listed host tools");
    assert!(names.contains(&format!("{ns}.ta")) && names.contains("host.whoami") && names.contains("host.quickstart"));
    assert!(!names.contains(&format!("{ns}.tb")), "a tool outside the allowlist must not be listed");

    for denied in ["host.secret.list", "host.key.rotate", &format!("{ns}.tb")] {
        let err = client
            .tools_call(denied, json!({}))
            .await
            .expect_err("a call outside the allowlist must be refused");
        assert_eq!(err.error_code.as_deref(), Some("permission_denied"), "{denied}: {err:?}");
        assert_eq!(err.data["allowed_tools"], json!(["ta"]), "{denied}");
        assert_eq!(err.data["key_label"], json!("scoped"), "{denied}");
    }

    // The allowlisted tool and the always-listed host tools still work.
    client.tools_call(&format!("{ns}.ta"), json!({})).await.expect("allowlisted call");
    client.tools_call("host.whoami", json!({})).await.expect("host.whoami");
}
