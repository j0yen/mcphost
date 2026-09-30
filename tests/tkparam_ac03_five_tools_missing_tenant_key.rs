//! PRD-mcphost-tenant-key-missing-is-invalid-params
//! AC3 (P0) — Given an argument-only call to each of `host.catalog.search`,
//! `host.catalog.get`, `host.state.table_create`, `host.state.insert` and
//! `host.agent.profile_set` with no `tenant_key`, When the server responds,
//! Then every one returns numeric code `-32602` with `data.error_code`
//! `tenant_key_missing` (one parameterized test, 5 cases).

use crate::common;
use common::{McpClient, TestServer};
use serde_json::{Value, json};

#[tokio::test]
async fn five_tools_refuse_missing_tenant_key_as_invalid_params() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    let cases: &[(&str, Value)] = &[
        ("host.catalog.search", json!({"q": "anything"})),
        ("host.catalog.get", json!({"full_name": "nobody.nothing"})),
        (
            "host.state.table_create",
            json!({"name": "t1", "schema": {"col": "text"}}),
        ),
        (
            "host.state.insert",
            json!({"table": "t1", "rows": [{"col": "x"}]}),
        ),
        ("host.agent.profile_set", json!({})),
    ];

    for (tool, args) in cases {
        let err = client
            .tools_call(tool, args.clone())
            .await
            .err()
            .unwrap_or_else(|| panic!("{tool} with no tenant_key must be refused"));
        assert_eq!(err.code, -32602, "{tool} must be INVALID_PARAMS: {err:?}");
        assert_eq!(
            err.error_code.as_deref(),
            Some("tenant_key_missing"),
            "{tool} must refuse as tenant_key_missing: {err:?}"
        );
    }
}
