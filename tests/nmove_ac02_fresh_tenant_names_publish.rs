//! PRD-mcphost-refusals-name-the-nearest-move
//! AC2 (P0) -- Given a fresh tenant with no tools, When it calls
//! `agent_self_register` keyed and then as an implicit tenant, Then both
//! `ToolNotFound` and `ToolNotFoundOnImplicitTenant` carry `tools_published`
//! `0`, a non-empty `nearest`, `did_you_mean` `[]`, `catalog_matches` `0` and
//! `next_call` `{tool: "host.tool_publish", args: {name:
//! "agent_self_register"}}`, and the implicit variant keeps `reason`,
//! `signup_source` and hint.

use crate::common::{McpClient, RpcError, TestServer, extract_structured, signup};
use serde_json::json;

fn assert_fresh_tenant_facts(err: &RpcError) {
    assert_eq!(err.error_code.as_deref(), Some("tool_not_found"), "{err:?}");
    assert_eq!(err.message, "tool not found: agent_self_register");
    let d = &err.data;
    assert_eq!(d["tools_published"], 0, "{d}");
    assert_eq!(d["catalog_matches"], 0, "{d}");
    assert_eq!(d["did_you_mean"], json!([]), "{d}");
    let nearest = d["nearest"].as_array().expect("nearest is an array");
    assert!(!nearest.is_empty() && nearest.len() <= 5, "{d}");
    assert_eq!(d["next_call"], json!({"tool": "host.tool_publish", "args": {"name": "agent_self_register"}}));
    assert_ne!(d["next_call"]["tool"], "host.tool.list", "an empty list is never the next move");
}

#[tokio::test]
async fn nmove_ac02() {
    let server = TestServer::start().await;

    // Keyed: a tenant that signed up and published nothing.
    let (_ns, key) = signup(&server.base_url, "Nmove02").await;
    let keyed = McpClient::with_bearer(&server.base_url, &key);
    let err = keyed.tools_call("agent_self_register", json!({})).await.expect_err("nothing published");
    assert_fresh_tenant_facts(&err);
    assert!(err.data.get("reason").is_none(), "a keyed tenant gets no implicit-tenant reason: {err:?}");

    // Implicit: the first host.* call makes the tenant; its personal URL is the connection.
    let first = McpClient::new(&server.base_url).with_session_continuity();
    let whoami = extract_structured(&first.tools_call("host.whoami", json!({})).await.expect("whoami"));
    let ns = whoami["onboarding"]["tenant"].as_str().expect("namespace").to_string();
    let url = whoami["onboarding"]["url"].as_str().expect("personal url");
    let implicit = McpClient::new(&server.base_url).with_path(url.strip_prefix(&server.base_url).expect("url on this server"));
    let err = implicit.tools_call("agent_self_register", json!({})).await.expect_err("nothing published");
    assert_fresh_tenant_facts(&err);
    assert_eq!(err.data["reason"], "tool_not_found_on_implicit_tenant", "{err:?}");
    assert_eq!(err.data["tenant"], ns.as_str());
    assert_eq!(err.data["signup_source"], "implicit");
    assert!(err.data["hint"].as_str().is_some_and(|h| h.contains("implicit sign-up")), "{err:?}");
}
