//! contract: reads registry write_verbs::DRY_RUN_VERBS
//! PRD-mcphost-dry-run-on-write-verbs
//! AC7 -- Given `tools/list` for a keyed tenant, When read, Then every
//! `Supported` verb's `inputSchema.properties.dry_run` exists with type
//! boolean and every `NoDryRun` verb's does not; the count of `Supported`
//! verbs equals `DRY_RUN_VERBS.len()`.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::write_verbs::{DRY_RUN_VERBS, DryRun, NO_DRY_RUN_VERBS, WRITE_VERBS};
use serde_json::Value;
use std::collections::BTreeMap;

#[tokio::test]
async fn tools_list_advertises_dry_run_exactly_on_the_supported_verbs() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Dryv AC7 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let listed = client.tools_list().await.expect("tools/list");
    let tools: BTreeMap<String, Value> = listed["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| (t["name"].as_str().expect("name").to_string(), t.clone()))
        .collect();

    let mut supported_seen = 0;
    for verb in WRITE_VERBS {
        let tool = tools.get(verb.name).unwrap_or_else(|| panic!("{} missing from tools/list", verb.name));
        let property = &tool["inputSchema"]["properties"]["dry_run"];
        match verb.dry_run {
            DryRun::Supported => {
                assert_eq!(property["type"], Value::String("boolean".into()), "{}: {tool}", verb.name);
                supported_seen += 1;
            }
            DryRun::NoDryRun(_) => {
                assert!(property.is_null(), "{} must not advertise dry_run: {property}", verb.name);
            }
        }
    }
    assert_eq!(supported_seen, DRY_RUN_VERBS.len());
    assert_eq!(WRITE_VERBS.len() - supported_seen, NO_DRY_RUN_VERBS.len());

    // The deprecated alias of a Supported verb carries it too (aliases are
    // clones of the canonical descriptor).
    let alias = &tools["host.tool_remove"];
    assert_eq!(alias["inputSchema"]["properties"]["dry_run"]["type"], Value::String("boolean".into()));
}
