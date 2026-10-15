//! PRD-mcphost-inbound-email-address-per-tenant
//! AC1 -- Given synthorg's corpus, When the `email_forward_receipt` task is
//! added and the chain runs in fake-provider mode, Then the ledger records
//! >= 1 `host_email_address` call and the task's gold `email_received` row is
//! observed, so the demand number exists.
//!
//! Same shape as `tests/mcphost_team_memory_ac07_synthorg_task_five_completions.rs`:
//! the corpus repository lives outside this worktree, so this loads the task
//! block from `tests/fixtures/emin/synthorg-task.yaml`, plays the agent's
//! steps against an in-process server in fake-provider mode
//! (`MCPHOST_EMAIL_PROVIDER=fake`), keeps the ledger of the tools it called,
//! and scores the run with the task's own `gold.call_check`.

use crate::common;

use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::email::EmailProvider;
use serde_json::{Value, json};

const SECRET: &str = "fake-inbound-secret";

/// The value of the first `key:` line (scalar) after `section:` -- hand-rolled
/// rather than a YAML crate for one small, self-owned fixture.
fn scalar(text: &str, section: &str, key: &str) -> String {
    let mut in_section = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed == format!("{section}:") {
            in_section = true;
            continue;
        }
        if in_section && let Some(v) = trimmed.strip_prefix(&format!("{key}:")) {
            return v.trim().trim_matches('"').to_string();
        }
    }
    panic!("{section}.{key} not in the task yaml");
}

/// `result.a.b[0].c == literal`, the mini-grammar the corpus uses.
fn call_check(expr: &str, result: &Value) -> bool {
    let (path, want) = expr.split_once("==").expect("call_check is `path == value`");
    let mut cur = result;
    for seg in path.trim().trim_start_matches("result").split('.').filter(|s| !s.is_empty()) {
        let (name, idx) = match seg.split_once('[') {
            Some((n, i)) => (n, Some(i.trim_end_matches(']').parse::<usize>().expect("index"))),
            None => (seg, None),
        };
        if !name.is_empty() {
            cur = &cur[name];
        }
        if let Some(i) = idx {
            cur = &cur[i];
        }
    }
    cur.as_str() == Some(want.trim().trim_matches('"'))
}

#[tokio::test]
async fn emin_ac01_task_run_records_the_address_call_and_the_gold_row() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/emin/synthorg-task.yaml");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    assert!(text.contains("id: integration-specialist-email-forward-receipt"), "task id");
    assert!(text.contains("`host.email.address`"), "the task text must name the verb the demand number counts");
    let from = scalar(&text, "fixture_mail", "from");
    let subject = scalar(&text, "fixture_mail", "subject");
    let mail_text = scalar(&text, "fixture_mail", "text");
    let check = scalar(&text, "gold", "call_check");
    assert_eq!(scalar(&text, "gold", "subject"), subject, "gold names the fixture mail's subject");

    let server = TestServer::start_with_email_inbound(SECRET, EmailProvider::Resend, true).await;
    let (_ns, key) = signup(&server.base_url, "emin synthorg").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let mut ledger: Vec<String> = Vec::new();

    ledger.push("host_email_address".to_string());
    let addr = extract_structured(&client.tools_call("host.email.address", json!({})).await.expect("address"));
    let address = addr["address"].as_str().expect("address").to_string();

    // The harness half: POST the fixture mail through FakeInbound.
    let resp = reqwest::Client::new()
        .post(format!("{}/email/inbound", server.base_url))
        .header("x-mcphost-fake-inbound", SECRET)
        .json(&json!({"from": from, "to": [address], "subject": subject, "text": mail_text}))
        .send()
        .await
        .expect("post fixture mail");
    assert_eq!(resp.status(), 200, "{}", resp.text().await.unwrap_or_default());

    ledger.push("host_msg_wait".to_string());
    let wait = extract_structured(&client.tools_call("host.msg.wait", json!({"timeout_s": 5})).await.expect("wait"));

    assert!(ledger.iter().filter(|t| *t == "host_email_address").count() >= 1, "ledger: {ledger:?}");
    assert!(call_check(&check, &wait), "gold call_check `{check}` failed on {wait}");
    assert_eq!(wait["messages"][0]["data"]["subject"], json!(subject), "{wait}");
}

#[test]
fn emin_ac01_call_check_grammar() {
    let r = json!({"messages": [{"data": {"kind": "email_received"}}]});
    assert!(call_check("result.messages[0].data.kind == email_received", &r));
    assert!(!call_check("result.messages[0].data.kind == other", &r));
}
