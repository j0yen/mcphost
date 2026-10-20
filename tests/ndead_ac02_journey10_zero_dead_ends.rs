//! PRD-mcphost-onboarding-dead-ends-zero AC2 (P0) -- Given the 10-call
//! fixture journey (`tests/fixtures/journey10.json`) with each wrong variant,
//! When it runs against an in-process host, Then every error carries
//! `next_call`, and executing each `next_call` as given returns success or a
//! different `error_code`.

use crate::common;
use common::{McpClient, extract_structured, signup};
use serde_json::{Value, json};

/// The error `data` of a `tools/call` response: JSON-RPC `error.data`, or the
/// structured content of an `isError` result.
fn error_data(resp: &Value) -> Option<Value> {
    if let Some(err) = resp.get("error") {
        return Some(err.get("data").cloned().unwrap_or(Value::Null));
    }
    let result = resp.get("result")?;
    (result.get("isError") == Some(&json!(true))).then(|| extract_structured(result))
}

async fn call(client: &McpClient, tool: &str, args: Value) -> Value {
    client.rpc("tools/call", json!({"name": tool, "arguments": args})).await
}

#[tokio::test]
async fn ndead_ac02_journey10_zero_dead_ends() {
    let journey: Value =
        serde_json::from_str(include_str!("fixtures/journey10.json")).expect("journey10.json parses");
    let steps = journey["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 10, "the journey is ten calls");

    let server = common::TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Ndead AC2 Tenant").await;
    let anonymous = McpClient::new(&server.base_url);
    let bound = McpClient::with_bearer(&server.base_url, &key);
    let bad_key = McpClient::with_bearer(&server.base_url, "mk_never_issued_by_this_host");
    let client_for = |who: &str| match who {
        "anonymous" => &anonymous,
        "bound" => &bound,
        "bad_key" => &bad_key,
        other => panic!("unknown auth {other}"),
    };

    let setup = &journey["setup_tool"];
    common::publish(
        &bound,
        setup["name"].as_str().expect("name"),
        setup["kind"].as_str().expect("kind"),
        setup["spec"].clone(),
    )
    .await;

    for step in steps {
        let n = step["n"].as_u64().expect("n");
        let tool = step["tool"].as_str().expect("tool");
        let resp = call(client_for(step["auth"].as_str().expect("auth")), tool, step["args"].clone()).await;
        let data = error_data(&resp).unwrap_or_else(|| panic!("step {n} ({tool}) was meant to fail: {resp}"));
        let code = data["error_code"].as_str().unwrap_or_else(|| panic!("step {n}: no error_code in {data}")).to_string();
        let next = data
            .get("next_call")
            .unwrap_or_else(|| panic!("step {n} ({tool}, {code}) carries no next_call: {data}"));
        let next_tool = next["tool"].as_str().unwrap_or_else(|| panic!("step {n}: next_call.tool missing: {next}"));
        let next_args = next["args"].clone();
        assert!(next_args.is_object(), "step {n}: next_call.args is an object: {next}");

        let follow = call(client_for(step["follow_auth"].as_str().expect("follow_auth")), next_tool, next_args).await;
        if let Some(again) = error_data(&follow) {
            let again_code = again["error_code"].as_str().unwrap_or_default();
            assert_ne!(
                again_code, code,
                "step {n}: next_call {next} loops back to {code}: {again}"
            );
            assert!(again.get("next_call").is_some(), "step {n}: the follow-up error has no next_call: {again}");
        }
    }
}
