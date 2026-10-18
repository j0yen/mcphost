//! PRD-mcphost-sampling-from-tools
//! AC1 (P0) — Given an in-process client advertising `sampling` whose
//! `createMessage` fixture answers `"yes"` with `usage.outputTokens: 1`, When
//! a python tool calls `mcphost.sampling.ask("Urgent?", max_tokens=4)` inside
//! `tools/call`, Then the tool receives `{text: "yes"}` and the call returns
//! in one round trip; Given a client that omits `sampling`, When the same
//! tool runs, Then it receives `sampling_unsupported {reason: "client",
//! alternative, help_url}` within 100 ms.
//!
//! The fixture is `support/sampling_session.rs`: a session-mode client playing
//! a client's `sampling/createMessage` callback. The 100 ms is measured by the
//! tool itself around the bridge call (`time.monotonic()`), so process spawn
//! and HTTP transport are not in the number.

use crate::common;
use crate::elicit_session;
use crate::sampling_session;
use common::{McpClient, TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use sampling_session::{Answer, SamplingSession};
use serde_json::{Value, json};
use std::time::Duration;

const TOOL: &str = "import mcphost, time\ndef main(args):\n    if args.get(\"warm\"):\n        return {\"warm\": True}\n    t = time.monotonic()\n    try:\n        r = mcphost.sampling.ask(\"Urgent?\", max_tokens=4)\n        return {\"text\": r[\"text\"], \"tokens\": r[\"tokens\"], \"model\": r[\"model\"]}\n    except mcphost.sampling.SamplingError as e:\n        return {\"code\": e.code, \"reason\": e.data.get(\"reason\"), \"alternative\": e.data.get(\"alternative\"), \"help_url\": e.data.get(\"help_url\"), \"ms\": int((time.monotonic() - t) * 1000)}\n";

/// Calls until the tool's environment is built.
async fn call_ready(client: &SamplingSession, name: &str, args: Value, answer: &Answer) -> Value {
    for _ in 0..200 {
        let resp = client.call(name, args.clone(), answer).await;
        let building = resp["result"]
            .get("structuredContent")
            .is_some_and(|s| s["status"] == json!("building"))
            || resp["error"]["data"]["error_code"] == json!("tool_building");
        if !building {
            return resp;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("tool never became ready");
}

#[tokio::test]
async fn python_ask_gets_the_client_answer_or_unsupported_client() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Msmp AC1").await;
    let admin = McpClient::with_bearer(&server.base_url, &key);
    admin
        .tools_call("host.tool_publish", json!({"name": "triage", "kind": "python", "spec": {"source": TOOL, "timeout_s": 40}}))
        .await
        .expect("publish");
    let name = format!("{ns}.triage");
    let session_base = elicit_session::serve_session_mode(server.state.clone()).await;
    let answer = Answer::text("yes").with_usage(0, 1);

    // A client that advertises sampling: the fixture's answer, in one call.
    let client = SamplingSession::open(&session_base, &key, true).await;
    call_ready(&client, &name, json!({"warm": true}), &answer).await;
    let resp = client.call(&name, json!({}), &answer).await;
    let out = extract_structured(&resp["result"]);
    assert_eq!(out["text"], json!("yes"), "{resp}");
    assert_eq!(out["tokens"], json!(1), "the client's reported usage is the count: {resp}");
    assert_eq!(client.received.lock().unwrap().len(), 1, "exactly one sampling/createMessage reached the client");
    let sent = client.received.lock().unwrap()[0].clone();
    assert_eq!(sent["params"]["maxTokens"], json!(4), "{sent}");
    assert_eq!(sent["params"]["messages"][0]["content"]["text"], json!("Urgent?"), "{sent}");

    // A client that omits sampling: the typed refusal, fast.
    let bare = SamplingSession::open(&session_base, &key, false).await;
    let resp = bare.call(&name, json!({}), &answer).await;
    let out = extract_structured(&resp["result"]);
    assert_eq!(out["code"], json!("sampling_unsupported"), "{resp}");
    assert_eq!(out["reason"], json!("client"), "{resp}");
    assert!(out["alternative"].as_str().is_some_and(|a| a.contains("prompt-kind")), "{resp}");
    assert!(out["help_url"].as_str().is_some_and(|u| u.starts_with("http")), "{resp}");
    let ms = out["ms"].as_u64().expect("ms");
    assert!(ms < 100, "refusal took {ms} ms: {resp}");
    assert_eq!(bare.received.lock().unwrap().len(), 0, "an unsupported client is never asked");
}
