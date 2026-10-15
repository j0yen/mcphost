//! PRD-mcphost-elicitation-with-ask-fallback
//! AC2 (P0) — Given a client without `elicitation`, When the same tool runs
//! and the ask-user page receives the answer within 5 s, Then the tool
//! receives `{action: "accept", content}` and the row says `channel: link`;
//! Given no answer within the wait, Then `{action: "pending", ask_id, url,
//! expires_unix}` is returned before the tool's own timeout.

use crate::common;
use common::{McpClient, TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

const TOOL: &str = "import mcphost\ndef main(args):\n    if args.get(\"warm\"):\n        return {\"warm\": True}\n    schema = {\"confirm\": {\"type\": \"boolean\"}}\n    resume = args.get(\"resume\")\n    if resume:\n        schema[\"_resume\"] = resume\n    return mcphost.elicit(\"Rotate?\", schema, timeout_s=args.get(\"timeout_s\"))\n";

struct Fixture {
    server: TestServer,
    client: Arc<McpClient>,
    ns: String,
    _envs: common::TempDataDir,
}

async fn fixture(label: &str) -> Fixture {
    let envs = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs.0)).await;
    let (ns, key) = signup(&server.base_url, label).await;
    let client = Arc::new(McpClient::with_bearer(&server.base_url, &key));
    client
        .tools_call("host.tool_publish", json!({"name": "rotate", "kind": "python", "spec": {"source": TOOL}}))
        .await
        .expect("publish");
    poll_until_ready(&client, &format!("{ns}.rotate"), json!({"warm": true}), Duration::from_secs(20))
        .await
        .expect("warm");
    Fixture { server, client, ns, _envs: envs }
}

async fn signed_url(f: &Fixture, ask_id: &str) -> String {
    let tenant = f.server.state.db.find_tenant_by_namespace(f.ns.clone()).await.expect("db").expect("tenant");
    let row = mcphost::asks::find(&f.server.state, ask_id).await.expect("find").expect("row");
    mcphost::asks::signed_ask_url(&f.server.base_url, &tenant.key_hash, ask_id, row.expires_unix)
}

async fn pending_ask_id(f: &Fixture) -> String {
    for _ in 0..100 {
        let listed = extract_structured(&f.client.tools_call("host.ask.list", json!({"status": "pending"})).await.expect("list"));
        if let Some(id) = listed["asks"][0]["ask_id"].as_str() {
            return id.to_string();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the elicit never created an ask");
}

#[tokio::test]
async fn answer_on_the_ask_page_within_the_wait_returns_accept_and_a_link_row() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let f = fixture("Melc AC2 answered").await;
    let caller = f.client.clone();
    let name = format!("{}.rotate", f.ns);
    let call = tokio::spawn(async move { caller.tools_call(&name, json!({})).await });

    let ask_id = pending_ask_id(&f).await;
    let answered = reqwest::Client::new()
        .post(signed_url(&f, &ask_id).await)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body("choice=accept")
        .send()
        .await
        .expect("post answer");
    assert_eq!(answered.status(), 200);

    let out = extract_structured(&call.await.expect("join").expect("tool call"));
    assert_eq!(out["action"], json!("accept"), "{out}");
    assert!(out["content"]["confirm"].is_boolean(), "{out}");

    let rows = extract_structured(&f.client.tools_call("host.ask.list", json!({})).await.expect("list"));
    assert_eq!(rows["asks"].as_array().expect("asks").len(), 1, "{rows}");
    assert_eq!(rows["asks"][0]["channel"], json!("link"), "{rows}");
    assert_eq!(rows["asks"][0]["status"], json!("answered"), "{rows}");
}

#[tokio::test]
async fn no_answer_returns_pending_with_the_url_before_the_tool_timeout_and_resumes_later() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let f = fixture("Melc AC2 pending").await;
    let out = crate::deadline_budget!(20_000, {
        extract_structured(
            &f.client
                .tools_call(&format!("{}.rotate", f.ns), json!({"timeout_s": 1}))
                .await
                .expect("tool call"),
        )
    });
    assert_eq!(out["action"], json!("pending"), "{out}");
    let ask_id = out["ask_id"].as_str().expect("ask_id").to_string();
    assert!(out["url"].as_str().is_some_and(|u| u.contains(&format!("/a/{ask_id}"))), "{out}");
    assert!(out["expires_unix"].as_i64().is_some(), "{out}");

    // The user answers later; a call carrying `schema._resume` returns the recorded answer.
    let answered = reqwest::Client::new()
        .post(signed_url(&f, &ask_id).await)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body("choice=accept")
        .send()
        .await
        .expect("post answer");
    assert_eq!(answered.status(), 200);
    let resumed: Value = extract_structured(
        &f.client
            .tools_call(&format!("{}.rotate", f.ns), json!({"resume": ask_id}))
            .await
            .expect("resume call"),
    );
    assert_eq!(resumed["action"], json!("accept"), "{resumed}");
    let rows = extract_structured(&f.client.tools_call("host.ask.list", json!({})).await.expect("list"));
    assert_eq!(rows["asks"].as_array().expect("asks").len(), 1, "resume reuses the one ask row: {rows}");
}
