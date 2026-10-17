//! PRD-mcphost-same-user-second-agent
//! AC5 — Given `CLIENT_INFO_NAMES`, When a join arrives with each table entry
//! as `clientInfo.name`, Then the new client's `label` starts with that
//! entry's surface id, and an unknown name yields `client-<n>`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::install_links::CLIENT_INFO_NAMES;
use serde_json::json;

#[tokio::test]
async fn sibl_ac05_labels_derive_from_client_info_names() {
    let server = TestServer::start().await;
    let base = server.base_url.clone();
    let (ns, key) = signup(&base, "Labelling Host").await;
    let first = McpClient::with_bearer(&base, &key);
    let me = extract_structured(&first.tools_call("host.whoami", json!({})).await.expect("whoami"));
    let join_path = me["join_url"].as_str().expect("join_url").trim_start_matches(&base).to_string();
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("lookup").expect("tenant");
    // One join per table entry plus the unknown one: lift the default cap of 5.
    server.state.db.set_join_code_max_uses(tenant.id, 100).await.expect("lift the cap");

    for (name, surface) in CLIENT_INFO_NAMES {
        let joiner = McpClient::new(&base).with_path(&join_path).with_client_info(name, "1.0").with_session_continuity();
        let joined = extract_structured(&joiner.tools_call("host.whoami", json!({})).await.expect("join"));
        let label = joined["onboarding"]["label"].as_str().expect("onboarding.label");
        assert!(label.starts_with(surface), "clientInfo.name {name:?}: label {label:?} must start with {surface:?}");
        let row = joined["clients"]
            .as_array()
            .expect("clients")
            .iter()
            .find(|c| c["this"] == json!(true))
            .expect("this row");
        assert_eq!(row["label"], json!(label), "the clients row carries the same label");
        assert_eq!(row["client_name"], json!(name));
    }

    let joiner = McpClient::new(&base).with_path(&join_path).with_client_info("mystery-agent", "0.1").with_session_continuity();
    let joined = extract_structured(&joiner.tools_call("host.whoami", json!({})).await.expect("join"));
    let label = joined["onboarding"]["label"].as_str().expect("onboarding.label");
    let n = label.strip_prefix("client-").unwrap_or_else(|| panic!("unknown name must label client-<n>: {label}"));
    assert!(n.parse::<u32>().is_ok(), "client-<n> with a numeric ordinal: {label}");
}
