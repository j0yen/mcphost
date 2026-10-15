//! PRD-mcphost-docs-telemetry-failed-after-search AC6 (P1) — Given the
//! operator key, When `host.help.gaps {days: 1}` is called, Then it returns
//! the `docs_gaps` shape plus `by_code`; Given a tenant key, Then
//! `forbidden`.

use crate::common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use mcphost::state::now_unix;
use serde_json::json;

#[tokio::test]
async fn dtf_ac06_operator_reads_gaps_with_by_code_tenant_is_forbidden() {
    let server = TestServer::start().await;
    let db = &server.state.db;
    let now = now_unix();
    db.insert_help_search_event_for_test(Some(1), "chain prev result", Some("docs/kinds/chain.md#mappings"), now - 60, Some("chain_mapping_unresolved"))
        .await
        .expect("seed");
    db.insert_help_search_event_for_test(Some(1), "chain prev result", Some("docs/kinds/chain.md#mappings"), now - 50, Some("chain_mapping_unresolved"))
        .await
        .expect("seed");
    db.insert_help_search_event_for_test(Some(2), "publish tool", None, now - 40, Some("invalid_spec"))
        .await
        .expect("seed");
    db.insert_help_search_event_for_test(Some(2), "outside one day", None, now - 2 * 86_400, Some("invalid_spec"))
        .await
        .expect("seed old");

    let operator = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let body = extract_structured(&operator.tools_call("host.help.gaps", json!({"days": 1})).await.expect("gaps"));
    assert_eq!(body["window_days"], json!(1), "{body}");
    assert_eq!(body["searches"], json!(3), "{body}");
    assert_eq!(body["followed_by_failure"], json!(3), "{body}");
    assert_eq!(body["top"][0]["query"], json!("chain prev result"), "{body}");
    assert_eq!(body["top"][0]["failures"], json!(2), "{body}");
    assert_eq!(body["top"][0]["top_code"], json!("chain_mapping_unresolved"), "{body}");
    assert_eq!(body["by_code"], json!({"chain_mapping_unresolved": 2, "invalid_spec": 1}), "{body}");

    let err = operator.tools_call("host.help.gaps", json!({"days": 31})).await.expect_err("days out of range");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{err:?}");

    let (_ns, key) = signup(&server.base_url, "DTF AC6").await;
    let tenant = McpClient::with_bearer(&server.base_url, &key);
    let err = tenant.tools_call("host.help.gaps", json!({})).await.expect_err("tenant key refused");
    assert_eq!(err.error_code.as_deref(), Some("forbidden"), "{err:?}");
}
