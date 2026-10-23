//! PRD-mcphost-schema-bound-agreement
//! contract: reads registry handler::host_tool_descriptors
//! AC4 (P0) -- Given a `TestServer` over `build_router`, When
//! `host.agent.contacts_import` gets `addresses: []` and then 51 addresses,
//! Then the validator refuses each `args_invalid` with `data.path`
//! `/addresses` and `data.constraint` set to the jsonschema keyword from the
//! validator error (`minItems`, then `maxItems`), and `handler_runs` stays 0.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::consent::MAX_IMPORT_ADDRESSES;
use mcphost::host_args::handler_runs;
use serde_json::json;

const TOOL: &str = "host.agent.contacts_import";

#[tokio::test]
async fn sba_ac04_empty_and_over_limit_address_lists_are_refused_with_path_and_keyword() {
    let server = TestServer::start_with_signup_rate_limit(100_000).await;
    let (ns, key) = signup(&server.base_url, "SBA AC4").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let over: Vec<String> = (0..=MAX_IMPORT_ADDRESSES).map(|i| format!("@sba_{i}")).collect();
    assert_eq!(over.len(), 51);
    for (addresses, keyword) in [(json!([]), "minItems"), (json!(over), "maxItems")] {
        let err = client.tools_call(TOOL, json!({"addresses": addresses})).await.expect_err("refused");
        assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{keyword}: {err:?}");
        assert_eq!(err.data.get("path"), Some(&json!("/addresses")), "{keyword} data.path");
        assert_eq!(err.data.get("constraint"), Some(&json!(keyword)), "{keyword} data.constraint");
        assert_eq!(handler_runs(&ns, TOOL), 0, "{keyword}: the handler must not run");
    }
}
