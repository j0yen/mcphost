//! PRD-mcphost-implicit-signup-key-in-hand
//! AC6 (P1) -- Given the yields and refusal of AC 3, When the journal,
//! `/contract.json` and the `initialize` result are read, Then each yield line
//! carries `peer_recent_yielded=true`, `session_presented=false`,
//! `mints_in_window=<n>` and the tool name, no line carries a key or URL,
//! `onboarding.peer_recent.mint_cap` is `3`,
//! `onboarding.implicit_signup.sessionless` is `"tenant_key_in_envelope"` and
//! `instructions` names `onboarding.tenant_key`.

use crate::common;
use common::{EnvGuard, McpClient, TestServer, extract_structured};
use mcphost::peer_recent::MINT_CAP_ENV;
use serde_json::{Value, json};

const UA: &str = "kih-ac06/agent";
const TOOL: &str = "host.table.create";

fn create_args() -> Value {
    json!({"name": "runs", "columns": {"id": "integer"}})
}

/// True when `text` holds `len` consecutive hex digits (the shape of a tenant key).
fn has_hex_run(text: &str, len: usize) -> bool {
    let mut run = 0;
    for c in text.chars() {
        run = if c.is_ascii_hexdigit() { run + 1 } else { 0 };
        if run >= len {
            return true;
        }
    }
    false
}

#[tokio::test]
async fn kih_ac06() {
    let server = {
        let _cap = EnvGuard::set(MINT_CAP_ENV, "3").await;
        TestServer::start_with_signup_rate_limit(1000).await
    };
    assert_eq!(server.state.redeemed_peers.cap(), 3, "the startup path read the env var");
    let journal = common::tracecap::capture_json();

    let mut keys = vec![];
    for n in 1..=3 {
        let created = extract_structured(
            &McpClient::new(&server.base_url)
                .with_user_agent(UA)
                .tools_call(TOOL, create_args())
                .await
                .unwrap_or_else(|e| panic!("mint {n}: {e:?}")),
        );
        keys.push(created["onboarding"]["tenant_key"].as_str().expect("tenant_key").to_string());
    }
    McpClient::new(&server.base_url)
        .with_user_agent(UA)
        .tools_call(TOOL, create_args())
        .await
        .expect_err("the fourth mint is refused");

    let lines = journal.json_lines();
    let fields = |v: &Value| v["fields"].clone();
    let yields: Vec<Value> = lines.iter().map(fields).filter(|f| f.get("peer_recent_yielded").is_some()).collect();
    let refusals: Vec<Value> = lines
        .iter()
        .map(fields)
        .filter(|f| f.get("session_presented").is_some() && f.get("peer_recent_yielded").is_none())
        .collect();

    assert_eq!(yields.len(), 2, "mints 2 and 3 are yields (mint 1 had no recent peer): {yields:?}");
    for (f, n) in yields.iter().zip([1, 2]) {
        assert_eq!(f["peer_recent_yielded"], json!(true), "{f}");
        assert_eq!(f["session_presented"], json!(false), "{f}");
        assert_eq!(f["mints_in_window"], json!(n), "{f}");
        assert_eq!(f["tool"], json!(TOOL), "{f}");
    }
    assert_eq!(refusals.len(), 1, "one refusal line: {refusals:?}");
    assert_eq!(refusals[0]["session_presented"], json!(false), "{}", refusals[0]);
    assert_eq!(refusals[0]["mints_in_window"], json!(3), "{}", refusals[0]);
    assert_eq!(refusals[0]["tool"], json!(TOOL), "{}", refusals[0]);

    for f in yields.iter().chain(refusals.iter()) {
        let text = f.to_string();
        for key in &keys {
            assert!(!text.contains(key.as_str()), "no tenant key: {text}");
        }
        assert!(!has_hex_run(&text, 64), "no 64-hex tenant key: {text}");
        assert!(!text.contains("/u/") && !text.contains("http"), "no URL: {text}");
    }
    let all_text = journal.text();
    for key in &keys {
        assert!(!all_text.contains(key.as_str()), "no journal line carries a tenant key");
    }

    let contract: Value = reqwest::Client::new()
        .get(format!("{}/contract.json", server.base_url))
        .send()
        .await
        .expect("GET /contract.json")
        .json()
        .await
        .expect("contract.json is JSON");
    assert_eq!(contract["onboarding"]["peer_recent"]["mint_cap"], json!(3), "{}", contract["onboarding"]);
    assert_eq!(
        contract["onboarding"]["implicit_signup"]["sessionless"],
        json!("tenant_key_in_envelope"),
        "{}",
        contract["onboarding"]
    );
    assert_eq!(contract["onboarding"]["implicit_signup"]["scope"], json!("session"), "unchanged");
    assert_eq!(contract["onboarding"]["peer_recent"]["window_secs"], json!(600), "unchanged");

    let init = McpClient::new(&server.base_url).initialize().await;
    let instructions = init["result"]["instructions"].as_str().expect("initialize result has instructions");
    assert!(instructions.contains("onboarding.tenant_key"), "{instructions}");
}
