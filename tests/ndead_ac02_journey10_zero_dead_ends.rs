//! PRD-mcphost-onboarding-dead-ends-zero
//! AC2 — Given the 10-call fixture journey with each wrong variant, When it
//! runs against an in-process host, Then every error carries `next_call`, and
//! executing each `next_call` as given returns success or a different
//! `error_code`.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::Value;

const JOURNEY: &str = include_str!("fixtures/journey10.json");

#[tokio::test]
async fn ndead_ac02() {
    let journey: Value = serde_json::from_str(JOURNEY).expect("journey10.json parses");
    let steps = journey["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 10, "the journey is ten calls");

    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Journey").await;
    let bound = McpClient::with_bearer(&server.base_url, &key);
    let anonymous = McpClient::new(&server.base_url);

    for step in steps {
        let name = step["step"].as_str().expect("step name");
        let tool = step["tool"].as_str().expect("tool");
        let client = if step["auth"] == "anonymous" { &anonymous } else { &bound };

        let err = client
            .tools_call(tool, step["wrong"].clone())
            .await
            .map(|ok| ok.to_string().chars().take(300).collect::<String>()).expect_err(&format!("{name}: the wrong variant must fail"));
        let wrong_code = err.error_code.clone().unwrap_or_default();
        let next = err.data.get("next_call").unwrap_or_else(|| panic!("{name}: {wrong_code} has no next_call: {}", err.data));
        let next_tool = next["tool"].as_str().unwrap_or_else(|| panic!("{name}: next_call.tool: {next}"));
        assert!(next["args"].is_object(), "{name}: next_call.args: {next}");
        eprintln!("STEP {name}: {wrong_code} -> {next}");

        match client.tools_call(next_tool, next["args"].clone()).await {
            Ok(_) => {}
            Err(e2) => {
                let code2 = e2.error_code.clone().unwrap_or_default();
                assert_ne!(code2, wrong_code, "{name}: next_call {next} loops on {wrong_code}");
                assert!(e2.data.get("next_call").is_some(), "{name}: follow-up error lacks next_call: {}", e2.data);
            }
        }

        let right_client = if step["right_auth"].as_str().unwrap_or(step["auth"].as_str().unwrap()) == "anonymous" {
            &anonymous
        } else {
            &bound
        };
        right_client
            .tools_call(tool, step["right"].clone())
            .await
            .unwrap_or_else(|e| panic!("{name}: the right call failed: {} {}", e.message, e.data));
    }
}
