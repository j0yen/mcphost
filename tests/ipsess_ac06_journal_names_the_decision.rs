//! PRD-mcphost-implicit-signup-per-session-not-per-peer
//! AC6 (P1) -- Given the refusals of AC 3 and the yield of AC 1, When the
//! journal is read, Then the refusal lines carry `session_presented=false` and
//! the yield line carries `peer_recent_yielded=true`, each with the tool name,
//! and no line carries a key or another tenant's namespace.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::{Value, json};

const UA: &str = "ipsess-ac06/agent";
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
async fn ipsess_ac06() {
    let server = TestServer::start().await;
    let journal = common::tracecap::capture_json();

    let a = McpClient::new(&server.base_url).with_user_agent(UA).with_session_continuity();
    a.initialize().await;
    let signup = extract_structured(
        &a.tools_call("signup", json!({"name": "ipsess-a", "handoff": true})).await.expect("A signup"),
    );
    let a_namespace = signup["tenant"].as_str().expect("tenant").to_string();
    let token = signup["handoff_token"].as_str().expect("handoff_token").to_string();
    a.tools_call("host.redeem", json!({"handoff_token": token})).await.expect("A redeem");

    // AC 3's two refusals: no header, and a forged id.
    McpClient::new(&server.base_url)
        .with_user_agent(UA)
        .tools_call(TOOL, create_args())
        .await
        .expect_err("sessionless is refused");
    McpClient::new(&server.base_url)
        .with_user_agent(UA)
        .tools_call_with_header(TOOL, create_args(), ("Mcp-Session-Id", "forged-never-minted-0123456789abcdef"))
        .await
        .expect_err("forged id is refused");

    // AC 1's yield: a second initialized session from the same peer.
    let b = McpClient::new(&server.base_url).with_user_agent(UA).with_session_continuity();
    b.initialize().await;
    b.tools_call(TOOL, create_args()).await.expect("B's keyless call is an implicit signup");

    let lines = journal.json_lines();
    let fields = |v: &Value| v["fields"].clone();
    let refusals: Vec<Value> = lines.iter().map(fields).filter(|f| f.get("session_presented").is_some()).collect();
    let yields: Vec<Value> = lines.iter().map(fields).filter(|f| f.get("peer_recent_yielded").is_some()).collect();

    assert_eq!(refusals.len(), 2, "one refusal line per refused request: {refusals:?}");
    for f in &refusals {
        assert_eq!(f["session_presented"], json!(false), "{f}");
        assert_eq!(f["tool"], json!(TOOL), "{f}");
    }
    assert_eq!(yields.len(), 1, "one yield line: {yields:?}");
    assert_eq!(yields[0]["peer_recent_yielded"], json!(true), "{}", yields[0]);
    assert_eq!(yields[0]["tool"], json!(TOOL), "{}", yields[0]);

    for f in refusals.iter().chain(yields.iter()) {
        let text = f.to_string();
        assert!(!text.contains(&a_namespace), "no other tenant's namespace: {text}");
        assert!(!text.contains(&token), "no handoff token: {text}");
        assert!(!has_hex_run(&text, 64), "no 64-hex tenant key: {text}");
    }
}
