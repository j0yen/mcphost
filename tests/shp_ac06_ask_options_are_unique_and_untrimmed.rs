//! PRD-mcphost-schema-shape-agreement
//! contract: reads registry handler::host_tool_descriptors
//! AC6 (P0) -- Given `host.ask.create`, When `options` gets two equal strings and
//! then two strings equal after trimming, Then the first is refused by the validator
//! with constraint `uniqueItems`, and the second is refused by the validator through
//! an `items` `pattern` forbidding leading/trailing whitespace or listed in
//! `SCHEMA_VALID_REFUSALS` with reason `normalised`.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::host_args::handler_runs;
use serde_json::json;

const TOOL: &str = "host.ask.create";

#[tokio::test]
async fn shp_ac06_duplicate_and_trim_equal_options_are_refused_by_the_validator() {
    let server = TestServer::start_with_signup_rate_limit(100_000).await;
    let (ns, key) = signup(&server.base_url, "SHP AC6").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call(TOOL, json!({"question": "q", "options": ["a", "a"]}))
        .await
        .expect_err("equal options are refused");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{err:?}");
    assert_eq!(err.data.get("path"), Some(&json!("/options")), "data.path");
    assert_eq!(err.data.get("constraint"), Some(&json!("uniqueItems")), "data.constraint");

    // `" a"` differs from `"a"` to `uniqueItems`; the `items` pattern forbidding surrounding
    // whitespace is what keeps the handler's trim-then-dedupe refusal unreachable.
    let err = client
        .tools_call(TOOL, json!({"question": "q", "options": ["a", " a"]}))
        .await
        .expect_err("trim-equal options are refused");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{err:?}");
    assert_eq!(err.data.get("path"), Some(&json!("/options/1")), "data.path");
    assert_eq!(err.data.get("constraint"), Some(&json!("pattern")), "data.constraint");
    assert_eq!(handler_runs(&ns, TOOL), 0, "the handler must not run for either call");

    let _ = client.tools_call(TOOL, json!({"question": "q", "options": ["a", "b c"]})).await;
    assert_eq!(handler_runs(&ns, TOOL), 1, "distinct, untrimmed options reach the handler");
}
