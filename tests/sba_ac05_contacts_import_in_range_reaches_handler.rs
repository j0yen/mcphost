//! PRD-mcphost-schema-bound-agreement
//! contract: reads registry handler::host_tool_descriptors
//! AC5 (P0) -- Given `host.agent.contacts_import` with 1 and with
//! `MAX_IMPORT_ADDRESSES` addresses, When dispatched, Then `handler_runs` rises
//! by one each, and the consent-layer import function (consent.rs:397 at the
//! cited tree) called directly with `[]` still returns its own refusal.

use crate::common;
use common::{McpClient, TestServer, bare_app_state, bare_tenant, signup};
use mcphost::consent::{MAX_IMPORT_ADDRESSES, contacts_import};
use mcphost::errors::AppError;
use mcphost::host_args::handler_runs;
use serde_json::json;

const TOOL: &str = "host.agent.contacts_import";

#[tokio::test]
async fn sba_ac05_one_and_the_maximum_address_lists_reach_the_handler() {
    let server = TestServer::start_with_signup_rate_limit(100_000).await;
    let (ns, key) = signup(&server.base_url, "SBA AC5").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    for n in [1, MAX_IMPORT_ADDRESSES] {
        let addresses: Vec<String> = (0..n).map(|i| format!("@sba_{i}")).collect();
        let before = handler_runs(&ns, TOOL);
        // The call may answer per-address results or a plan refusal; the validator must let it through.
        let _ = client.tools_call(TOOL, json!({"addresses": addresses})).await;
        assert_eq!(handler_runs(&ns, TOOL), before + 1, "{n} addresses must reach the handler");
    }
}

#[tokio::test]
async fn sba_ac05_consent_layer_keeps_its_own_empty_refusal() {
    let (state, _dir) = bare_app_state().await;
    let tenant = bare_tenant(&state, "t_sba_ac05").await;
    let err = contacts_import(&state, &tenant, &json!({"addresses": []})).await.expect_err("empty refused");
    assert!(matches!(&err, AppError::InvalidArgs(m) if m.contains("at least one address")), "{err:?}");
}
