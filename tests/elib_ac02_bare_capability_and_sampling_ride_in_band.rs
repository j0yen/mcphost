//! PRD-mcphost-elicitation-in-band-on-stateless-transport
//! AC2 (P0) — Given AC 1's client declaring the bare `{"elicitation":{}}`,
//! When the tool elicits, Then AC 1's in-band path is taken; Given a
//! `TestServer` client declaring `sampling` that answers
//! `sampling/createMessage` with the fixture text, When a tool calls
//! `mcphost.sampling.ask`, Then the frame rides the call's own stream and the
//! tool gets that text; a parser table test covers `{}`/`{"form":{}}` → form,
//! `{"url":{}}`/absent → link, `sampling` present/absent.

use crate::inband_client;

use crate::common;
use common::{McpClient, TestServer, extract_structured, python_kind_registry, signup};
use inband_client::{RawClient, accept, sampled};
use mcphost::inband::{ElicitShape, elicitation_shape, form_elicitation_advertised, sampling_advertised};
use mcphost::sandbox;
use rmcp::model::ClientCapabilities;
use serde_json::{Value, json};
use std::time::Duration;

const ELICIT_TOOL: &str = "import mcphost\ndef main(args):\n    return mcphost.elicit(\"Rotate?\", {\"confirm\": {\"type\": \"boolean\"}})\n";
const SAMPLE_TOOL: &str = "import mcphost\ndef main(args):\n    if args.get(\"warm\"):\n        return {\"warm\": True}\n    r = mcphost.sampling.ask(\"Urgent?\", max_tokens=4)\n    return {\"text\": r[\"text\"]}\n";

fn caps(v: Value) -> ClientCapabilities {
    serde_json::from_value(v).expect("capabilities parse")
}

#[test]
fn parser_table_reads_form_url_and_sampling_from_one_function() {
    let table = [
        (json!({"elicitation": {}}), ElicitShape::Empty, true),
        (json!({"elicitation": {"form": {}}}), ElicitShape::Form, true),
        (json!({"elicitation": {"form": {}, "url": {}}}), ElicitShape::Form, true),
        (json!({"elicitation": {"url": {}}}), ElicitShape::Url, false),
        (json!({}), ElicitShape::None, false),
    ];
    for (raw, shape, form) in table {
        let parsed = caps(raw.clone());
        assert_eq!(elicitation_shape(&parsed), shape, "{raw}");
        assert_eq!(form_elicitation_advertised(&parsed), form, "{raw}");
    }
    assert!(sampling_advertised(&caps(json!({"sampling": {}}))));
    assert!(!sampling_advertised(&caps(json!({}))));
    assert!(!sampling_advertised(&caps(json!({"elicitation": {}}))));
}

async fn publish(server: &TestServer, label: &str, name: &str, source: &str) -> (String, McpClient, String) {
    let (ns, key) = signup(&server.base_url, label).await;
    let admin = McpClient::with_bearer(&server.base_url, &key);
    admin
        .tools_call("host.tool_publish", json!({"name": name, "kind": "python", "spec": {"source": source, "timeout_s": 40}}))
        .await
        .expect("publish");
    (format!("{ns}.{name}"), admin, key)
}

/// Calls until the tool's environment is built.
async fn ready(client: &RawClient, name: &str, args: Value, answer: &(dyn Fn(&Value) -> inband_client::Answer + Sync)) -> inband_client::Outcome {
    for _ in 0..200 {
        let out = client.call(name, args.clone(), answer).await;
        let building = out.response["result"].get("structuredContent").is_some_and(|s| s["status"] == json!("building"))
            || out.response["error"]["data"]["error_code"] == json!("tool_building");
        if !building {
            return out;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("tool never became ready");
}

#[tokio::test]
async fn bare_elicitation_capability_takes_the_in_band_path() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (name, admin, key) = publish(&server, "Elib AC2 bare", "rotate", ELICIT_TOOL).await;
    let client = RawClient::open(&server.base_url, &key, json!({"elicitation": {}}), true).await;
    let out = ready(&client, &name, json!({}), &|_| accept(json!({"confirm": true}))).await;
    assert_eq!(out.frames.len(), 1, "{:?}", out.frames);
    assert_eq!(out.frames[0]["method"], json!("elicitation/create"));
    assert_eq!(
        extract_structured(&out.response["result"]),
        json!({"action": "accept", "content": {"confirm": true}}),
        "{}",
        out.response
    );
    let rows = extract_structured(&admin.tools_call("host.ask.list", json!({})).await.expect("ask list"));
    assert_eq!(rows["asks"][0]["channel"], json!("client"), "{rows}");
}

#[tokio::test]
async fn sampling_frame_rides_the_calls_own_stream_and_the_tool_gets_the_text() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (name, _admin, key) = publish(&server, "Elib AC2 sampling", "triage", SAMPLE_TOOL).await;
    let client = RawClient::open(&server.base_url, &key, json!({"sampling": {}}), true).await;
    let answer = |_: &Value| sampled("fixture says yes");
    ready(&client, &name, json!({"warm": true}), &answer).await;
    let out = client.call(&name, json!({}), &answer).await;
    assert_eq!(out.content_type, "text/event-stream", "a sampling call streams");
    assert_eq!(out.frames.len(), 1, "{:?}", out.frames);
    assert_eq!(out.frames[0]["method"], json!("sampling/createMessage"));
    assert_eq!(out.frames[0]["params"]["messages"][0]["content"]["text"], json!("Urgent?"));
    assert_eq!(extract_structured(&out.response["result"])["text"], json!("fixture says yes"), "{}", out.response);
}
