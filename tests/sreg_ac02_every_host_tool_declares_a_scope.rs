//! PRD-mcphost-scope-registry-generated
//! AC2 (P0) — Given `host_tools(kinds, true)`, When the R2 test counts tools
//! without a `scope`, Then the count is 0 and every `tools/list` entry
//! carries `_meta.scope` naming a `Scope::ALL` member.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::scopes::{self, Scope};

#[tokio::test]
async fn sreg_ac02_tools_list_entries_all_carry_a_registry_scope() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Scope Registry Tools Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let list = client.tools_list().await.expect("tools/list");
    let tools = list["tools"].as_array().expect("tools array");
    assert!(!tools.is_empty());
    let mut missing = Vec::new();
    for t in tools {
        let name = t["name"].as_str().unwrap_or("?");
        match t["_meta"]["scope"].as_str().and_then(Scope::parse) {
            Some(_) => {}
            None => missing.push(name.to_string()),
        }
    }
    assert!(missing.is_empty(), "tools without a registry _meta.scope: {missing:?}");
}

#[test]
fn sreg_ac02_verb_scope_is_a_registry_member_for_every_name() {
    for name in ["host.whoami", "host.key.rotate", "host.secret.set", "host.tool.publish", "host.tool.list", "billing.status"] {
        assert!(Scope::ALL.contains(&scopes::verb_scope(name)), "{name}");
    }
}
