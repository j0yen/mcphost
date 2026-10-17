//! PRD-mcphost-delegation-recipes-and-quickstart
//! AC2 (P0) — Given a keyed tenant on a plan with `delegated_keys_max > 0`,
//! When `host.quickstart` runs, Then `recipes.sub_agent_key.calls` has length
//! 3 and `next_steps` names `delegate a key` at a lower index than `share a
//! tool`; Given an anonymous caller, Then the response is unchanged.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

fn step_index(response: &Value, name: &str) -> usize {
    response["next_steps"]
        .as_array()
        .expect("next_steps array")
        .iter()
        .position(|s| s["step"] == name)
        .unwrap_or_else(|| panic!("{name} not in next_steps: {response}"))
}

#[tokio::test]
async fn dlrec_ac02_keyed_quickstart_carries_the_recipe_and_orders_next_steps() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Recipe Quickstart").await;
    let keyed = McpClient::with_bearer(&server.base_url, &key);
    let out = extract_structured(&keyed.tools_call("host.quickstart", json!({"kind": "echo"})).await.expect("quickstart"));

    let calls = out["recipes"]["sub_agent_key"]["calls"].as_array().expect("calls");
    assert_eq!(calls.len(), 3, "{out}");
    assert_eq!(calls[0]["tool"], json!("host.key.delegate"));
    assert!(
        step_index(&out, "delegate a key") < step_index(&out, "share a tool"),
        "delegate must precede share: {}",
        out["next_steps"]
    );
    assert!(
        out["recipes"]["names"].as_array().is_some_and(|n| n.iter().any(|r| r == "webhook-inbox")),
        "existing recipe names stay: {}",
        out["recipes"]
    );
    let copy = out["recipes"]["sub_agent_key"]["copy_paste"].as_str().expect("copy_paste");
    assert_eq!(copy.matches("curl -s").count(), 3, "{copy}");
    // Size guardrail: the recipe adds at most 2 KB to the response.
    let added = serde_json::to_string(&out["recipes"]["sub_agent_key"]).unwrap().len()
        + serde_json::to_string(&out["next_steps"]).unwrap().len();
    assert!(added <= 2048, "recipe + next_steps add {added} bytes");
}

#[tokio::test]
async fn dlrec_ac02_a_plan_without_delegation_and_an_anonymous_caller_get_no_recipe() {
    let server = TestServer::start_with_plans(|catalog| {
        for p in &mut catalog.plans {
            p.delegated_keys_max = 0;
        }
    })
    .await;
    let (_ns, key) = signup(&server.base_url, "No Delegation").await;
    let keyed = McpClient::with_bearer(&server.base_url, &key);
    let out = extract_structured(&keyed.tools_call("host.quickstart", json!({"kind": "echo"})).await.expect("quickstart"));
    assert!(out["recipes"].is_array(), "{}", out["recipes"]);
    assert!(out.get("next_steps").is_none(), "{out}");

    let anon = McpClient::new(&server.base_url);
    let out = extract_structured(&anon.tools_call("host.quickstart", json!({})).await.expect("anon quickstart"));
    assert_eq!(out["authenticated"], json!(false));
    assert!(out.get("next_steps").is_none(), "{out}");
    assert_eq!(out["recipes"], json!(mcphost::kinds::aliases::recipe_names()));
    assert!(!out.to_string().contains("sub_agent_key"), "anonymous response unchanged");
}
