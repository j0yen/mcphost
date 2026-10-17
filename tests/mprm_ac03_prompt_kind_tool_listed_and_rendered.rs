//! AC3 (P0) -- PRD-mcphost-prompts-from-published-tools. With a `prompt`
//! kind registered and a tenant holding one prompt-kind tool (`text`
//! required, `labels` optional), `prompts/list` with the tenant key shows it
//! with those arguments; `prompts/get` returns the rendered template, never
//! reaches the provider and leaves `host.usage` unchanged.

use crate::common;
use crate::promptkind;

use common::{McpClient, TestServer, extract_structured, signup};
use promptkind::{PromptKind, classify_spec};
use serde_json::json;

#[tokio::test]
async fn mprm_ac03_prompt_kind_tool_listed_and_rendered() {
    let kind = PromptKind::new();
    let server = TestServer::start_with_kinds(kind.registry()).await;
    let (_ns, key) = signup(&server.base_url, "mprm ac03").await;
    let tenant = McpClient::with_bearer(&server.base_url, &key);
    tenant
        .tools_call("host.tool_publish", json!({"name": "classify_ticket", "kind": "prompt", "spec": classify_spec()}))
        .await
        .expect("publish prompt-kind tool");

    // Anonymous: recipes only.
    let anon = McpClient::new(&server.base_url);
    let anon_list = anon.rpc("prompts/list", json!({})).await;
    assert!(
        !anon_list["result"]["prompts"].as_array().expect("prompts").iter().any(|p| p["name"] == "classify_ticket"),
        "{anon_list}"
    );

    let listed = tenant.rpc("prompts/list", json!({})).await;
    let prompts = listed["result"]["prompts"].as_array().unwrap_or_else(|| panic!("{listed}"));
    let tool = prompts.iter().find(|p| p["name"] == "classify_ticket").unwrap_or_else(|| panic!("tool missing: {listed}"));
    let args = tool["arguments"].as_array().expect("arguments");
    let shape: Vec<(String, bool)> =
        args.iter().map(|a| (a["name"].as_str().unwrap().to_string(), a["required"].as_bool().unwrap_or(false))).collect();
    assert_eq!(shape, vec![("text".to_string(), true), ("labels".to_string(), false)], "{tool}");
    assert!(prompts.len() > 1, "recipes still listed beside the tool: {listed}");

    let usage_before = extract_structured(&tenant.tools_call("host.usage", json!({"window": "24h"})).await.expect("usage"))["calls"].clone();
    let got = tenant
        .rpc("prompts/get", json!({"name": "classify_ticket", "arguments": {"text": "printer on fire", "labels": "urgent,hardware"}}))
        .await;
    let text = got["result"]["messages"][0]["content"]["text"].as_str().unwrap_or_else(|| panic!("{got}"));
    assert_eq!(text, "Classify this ticket: printer on fire using the labels: urgent,hardware");
    assert_eq!(got["result"]["messages"][0]["role"], "user");
    assert_eq!(kind.provider_calls(), 0, "prompts/get must not reach the provider");
    let usage_after = extract_structured(&tenant.tools_call("host.usage", json!({"window": "24h"})).await.expect("usage"))["calls"].clone();
    assert_eq!(usage_before, usage_after, "host.usage unchanged by prompts/get");

    let missing = tenant.rpc("prompts/get", json!({"name": "classify_ticket", "arguments": {"labels": "x"}})).await;
    assert_eq!(missing["error"]["code"], -32602, "{missing}");
    assert_eq!(missing["error"]["data"]["argument"], "text", "{missing}");
}
