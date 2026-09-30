//! PRD-mcphost-session-bound-tenant-after-signup
//! AC12 (P2) — Given `tools/list` on an anonymous connection, When the
//! `tenant_key` description is read on each of the six tools, Then it states
//! the key is optional on the connection that ran `signup`, and
//! `contracts/host-tools.v1.json` matches `mcphost contract dump`
//! byte-for-byte.
//!
//! "The six tools" are the ones PRD-mcphost-session-key's AC3 named and
//! `tkparam_ac09_schema_tenant_key_not_required_and_discoverable.rs` still
//! pins; every `host.*` descriptor shares one generated description
//! (`handler::host_schema`), so the assertion runs over all of them rather
//! than a sample.

use crate::common;
use common::{McpClient, TestServer};
use mcphost::api_contract::dump_contract_bytes;
use mcphost::kinds::KindRegistry;
use serde_json::Value;

const SIX_TOOLS: &[&str] = &[
    "host.tool_publish",
    "host.catalog.get",
    "host.catalog.search",
    "host.state.table_create",
    "host.state.insert",
    "host.agent.profile_set",
];

/// The claim AC12 requires, in the exact words the description uses.
const OPTIONALITY_CLAIM: &str = "Optional on the connection that ran signup";

#[tokio::test]
async fn every_host_tools_tenant_key_description_states_signup_connection_optionality() {
    let server = TestServer::start().await;
    let anonymous = McpClient::new(&server.base_url);

    let listed = anonymous.tools_list().await.expect("anonymous tools/list");
    let tools = listed["tools"].as_array().expect("tools array");

    for name in SIX_TOOLS {
        let tool = tools
            .iter()
            .find(|t| t["name"].as_str() == Some(name))
            .unwrap_or_else(|| panic!("anonymous tools/list must still list {name}"));
        let description = tool["inputSchema"]["properties"]["tenant_key"]["description"]
            .as_str()
            .unwrap_or_else(|| panic!("{name} must carry a tenant_key description: {tool}"));
        assert!(
            description.contains(OPTIONALITY_CLAIM),
            "{name}'s tenant_key description must state the key is optional on the connection \
             that ran signup: {description}"
        );
        // The pre-existing conditions it must not lose: the header still
        // wins, and a connection with neither still gets tenant_key_missing.
        assert!(
            description.contains("the header wins")
                && description.contains("tenant_key_missing"),
            "{name}'s description must keep the header-wins and tenant_key_missing \
             conditions: {description}"
        );
    }

    // Every host.* tool shares the generated description, so none of them may
    // be silently left behind.
    for tool in tools {
        let Some(name) = tool["name"].as_str() else { continue };
        if !name.starts_with("host.") {
            continue;
        }
        let Some(description) = tool["inputSchema"]["properties"]["tenant_key"]["description"]
            .as_str()
        else {
            continue;
        };
        assert!(
            description.contains(OPTIONALITY_CLAIM),
            "{name} shares host_schema's tenant_key description and must carry the claim too: \
             {description}"
        );
    }
}

/// AC12's second half: the committed contract snapshot is regenerated from
/// the same description, so the two can never drift.
#[test]
fn the_committed_contract_matches_contract_dump_and_carries_the_same_claim() {
    const COMMITTED: &[u8] = include_bytes!("../contracts/host-tools.v1.json");
    let dumped = dump_contract_bytes(&KindRegistry::with_builtin());
    assert_eq!(
        dumped, COMMITTED,
        "contracts/host-tools.v1.json is stale -- regenerate with `mcphost contract dump`"
    );

    let contract: Value =
        serde_json::from_slice(COMMITTED).expect("the committed contract is valid JSON");
    let text = String::from_utf8_lossy(COMMITTED);
    assert!(
        text.contains(OPTIONALITY_CLAIM),
        "the committed contract must carry the same signup-connection optionality claim"
    );
    // And it is on the tenant_key property specifically, not just somewhere
    // in the file.
    let tools = contract["tools"].as_array().expect("contract tools array");
    for name in SIX_TOOLS {
        let tool = tools
            .iter()
            .find(|t| t["name"].as_str() == Some(name))
            .unwrap_or_else(|| panic!("the contract must list {name}"));
        // The committed contract spells it `input_schema` (serde's own
        // snake_case field name); the live `tools/list` wire shape spells it
        // `inputSchema`. Same description either way.
        let description = tool["input_schema"]["properties"]["tenant_key"]["description"]
            .as_str()
            .unwrap_or_else(|| panic!("{name} must carry a tenant_key description in the contract"));
        assert!(description.contains(OPTIONALITY_CLAIM), "{name}: {description}");
    }
}
