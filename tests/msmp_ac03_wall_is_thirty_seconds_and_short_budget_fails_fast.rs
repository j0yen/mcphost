//! PRD-mcphost-sampling-from-tools
//! AC3 (P0) — Given a fixture client that answers after 40 s, When the tool
//! asks with 50 s remaining, Then `sampling_timeout` is raised at
//! 30 000-30 500 ms; Given 10 s remaining, Then `sampling_timeout
//! {remaining_ms}` is raised before the client is asked.
//!
//! "N s remaining" is the tool's own `timeout_s` (60 and 10). The 30 s is
//! measured by the tool around the bridge call (`time.monotonic()`), so the
//! sandbox spawn and HTTP transport are not in the number.

use crate::common;
use crate::elicit_session;
use crate::sampling_session;
use common::{McpClient, TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use sampling_session::{Answer, SamplingSession};
use serde_json::{Value, json};
use std::time::Duration;

const TOOL: &str = "import mcphost, time\ndef main(args):\n    if args.get(\"warm\"):\n        return {\"warm\": True}\n    t = time.monotonic()\n    try:\n        return mcphost.sampling.ask(\"Urgent?\", max_tokens=4)\n    except mcphost.sampling.SamplingError as e:\n        return {\"code\": e.code, \"data\": e.data, \"ms\": int((time.monotonic() - t) * 1000)}\n";

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
async fn an_ask_is_cut_at_thirty_seconds_and_a_short_budget_never_asks() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Msmp AC3").await;
    let admin = McpClient::with_bearer(&server.base_url, &key);
    for (name, timeout_s) in [("roomy", 60), ("tight", 10)] {
        admin
            .tools_call(
                "host.tool_publish",
                json!({"name": name, "kind": "python", "spec": {"source": TOOL, "timeout_s": timeout_s}}),
            )
            .await
            .expect("publish");
    }
    let base = elicit_session::serve_session_mode(server.state.clone()).await;
    let client = SamplingSession::open(&base, &key, true).await;
    let slow = Answer::text("yes").with_usage(0, 1).after(Duration::from_secs(40));
    let (roomy, tight) = (format!("{ns}.roomy"), format!("{ns}.tight"));
    call_ready(&client, &roomy, json!({"warm": true}), &slow).await;
    call_ready(&client, &tight, json!({"warm": true}), &slow).await;

    // 10 s remaining: refused before the client is asked.
    let resp = client.call(&tight, json!({}), &slow).await;
    let out = extract_structured(&resp["result"]);
    assert_eq!(out["code"], json!("sampling_timeout"), "{resp}");
    let remaining_ms = out["data"]["remaining_ms"].as_u64().expect("remaining_ms");
    assert!(remaining_ms > 0 && remaining_ms <= 10_000, "remaining_ms must be what is left of 10 s: {resp}");
    assert_eq!(out["data"]["asked"], json!(false), "{resp}");
    assert_eq!(client.received.lock().unwrap().len(), 0, "the client must not be asked: {resp}");

    // 50+ s remaining: asked, and cut at the 30 s wall though the client needs 40.
    let resp = client.call(&roomy, json!({}), &slow).await;
    let out = extract_structured(&resp["result"]);
    assert_eq!(out["code"], json!("sampling_timeout"), "{resp}");
    assert_eq!(out["data"]["asked"], json!(true), "{resp}");
    let ms = out["ms"].as_u64().expect("ms");
    assert!((30_000..=30_500).contains(&ms), "the wall is 30 s, got {ms} ms: {resp}");
    assert_eq!(client.received.lock().unwrap().len(), 1, "the client was asked once");
}
