//! PRD-mcphost-acting-for-in-ledger-and-audit
//! AC7 (P0) — Given a serialized `acting_for`, When a test inspects its
//! keys, Then they are exactly `key_id, label, parent_label, root_label,
//! depth` and no value matches `^mk_`.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn assert_shape(af: &Value, minted_key: &str) {
    let keys: BTreeSet<&str> = af.as_object().expect("object").keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        BTreeSet::from(["key_id", "label", "parent_label", "root_label", "depth"]),
        "{af}"
    );
    for (k, v) in af.as_object().unwrap() {
        let s = v.to_string();
        assert!(!v.as_str().is_some_and(|s| s.starts_with("mk_")), "{k} looks like a key: {s}");
        assert!(!s.contains(minted_key), "{k} leaks the key: {s}");
    }
}

#[test]
fn actfor_ac07_serialized_shape_is_exactly_five_fields() {
    let af = mcphost::subkeys::ActingFor {
        key_id: 3,
        label: "deployer".into(),
        parent_label: Some("lead-job-42".into()),
        root_label: "lead-job-42".into(),
        depth: 2,
    };
    assert_shape(&af.to_json(), "mk_unused");
}

#[tokio::test]
async fn actfor_ac07_served_acting_for_has_exactly_the_five_fields_and_no_key_material() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "ActFor Redaction Tenant").await;
    let primary = McpClient::with_bearer(&server.base_url, &key);
    let out: Value = extract_structured(
        &primary
            .tools_call("host.key.delegate", json!({"scopes": ["tools:call"], "ttl_s": 600, "label": "deployer"}))
            .await
            .expect("delegate"),
    );
    let minted = out["key"].as_str().expect("key").to_string();
    let child = McpClient::with_bearer(&server.base_url, &minted);

    let ok = child.tools_call("host.whoami", json!({})).await.expect("whoami");
    assert_shape(&ok["_meta"]["acting_for"], &minted);
    let err = child.tools_call("no_such_tool_here", json!({})).await.expect_err("unknown tool");
    assert_shape(&err.data["acting_for"], &minted);
}
