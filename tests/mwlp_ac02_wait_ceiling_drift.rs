//! PRD-mcphost-msg-wait-long-poll-contract
//! contract: walks src/
//! AC2 (P0) — Given `quickstart_limits` and both wait descriptors, When the
//! drift test runs, Then `limits.wait_max_s == longpoll::WAIT_MAX_S`,
//! `limits.wait_default_s == WAIT_DEFAULT_S`, and both descriptions contain
//! the rendered `WAIT_MAX_S`. `/plans.json` and `www/llms.txt` carry the
//! same numbers.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::longpoll::{WAIT_DEFAULT_S, WAIT_MAX_S};
use serde_json::{Value, json};

fn description_of(tools: &Value, name: &str) -> String {
    let d = tools["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .find(|t| t["name"] == json!(name))
        .unwrap_or_else(|| panic!("{name} must be listed"))["description"]
        .as_str()
        .expect("description")
        .to_string();
    d.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[tokio::test]
async fn quickstart_plans_descriptors_and_llms_txt_carry_the_wait_constants() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "mwlp ac02 tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let quickstart = extract_structured(&client.tools_call("host.quickstart", json!({"kind": "echo"})).await.expect("quickstart"));
    assert_eq!(quickstart["limits"]["wait_max_s"], json!(WAIT_MAX_S), "{quickstart}");
    assert_eq!(quickstart["limits"]["wait_default_s"], json!(WAIT_DEFAULT_S), "{quickstart}");

    let tools = client.tools_list().await.expect("tools/list");
    let ceiling = format!("max {WAIT_MAX_S}s");
    for name in ["host.msg.wait", "host.runs.wait"] {
        let d = description_of(&tools, name);
        assert!(d.contains(&ceiling), "{name} descriptor must say '{ceiling}': {d}");
    }

    let plans: Value = reqwest::get(format!("{}/plans.json", server.base_url))
        .await
        .expect("GET /plans.json")
        .json()
        .await
        .expect("plans json");
    let plan_rows = plans["plans"].as_array().expect("plans array");
    assert!(!plan_rows.is_empty());
    for plan in plan_rows {
        assert_eq!(plan["wait_max_s"], json!(WAIT_MAX_S), "{plan}");
        assert_eq!(plan["wait_default_s"], json!(WAIT_DEFAULT_S), "{plan}");
    }

    let llms = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/www/llms.txt")).expect("read llms.txt");
    let line = format!("block for at most {WAIT_MAX_S} s (`limits.wait_max_s`");
    assert!(llms.contains(&line), "www/llms.txt must carry '{line}'");
    assert!(llms.contains(&format!("default {WAIT_DEFAULT_S} s")), "www/llms.txt must carry the default");
}
