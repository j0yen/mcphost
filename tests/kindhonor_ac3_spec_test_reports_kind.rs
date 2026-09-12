//! AC3 -- Given `host.spec_test` on any spec, When it runs, Then the report
//! includes `kind.resolved` and, if a kind was requested, `kind.requested`
//! with a `reason` on disagreement.
//!
//! Uses the `echo` kind alone (no sandbox/network dependency): its own
//! `validate` only requires `spec.schema`, so a spec that *also* carries an
//! unrelated `source` field (python's own required field) still validates
//! successfully under `echo` while structurally implying `python` -- the one
//! case that reaches `host.spec_test`'s success path with a genuine
//! kind disagreement to report (a real `http`-vs-`python` disagreement always
//! fails each kind's own required-field validation first, the same as
//! `host.tool_publish`, and never reaches this report).

use crate::common;
use common::{TestServer, signup};
use mcphost::kinds::KindRegistry;
use serde_json::json;

#[tokio::test]
async fn matching_kind_reports_resolved_with_no_reason() {
    let server = TestServer::start_with_kinds(KindRegistry::with_builtin()).await;
    let (_ns, key) = signup(&server.base_url, "Kind Honor AC3a").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let result = client
        .tools_call(
            "host.spec_test",
            json!({
                "kind": "echo",
                "spec": {"schema": {"type": "object"}},
                "invocations": [],
            }),
        )
        .await
        .expect("host.spec_test on a plain echo spec succeeds");
    let structured = common::extract_structured(&result);
    assert_eq!(structured["kind"]["resolved"], json!("echo"));
    assert_eq!(structured["kind"]["requested"], json!("echo"));
    assert!(
        structured["kind"].get("reason").is_none(),
        "no reason expected when resolved and requested agree: {:?}",
        structured["kind"]
    );
}

#[tokio::test]
async fn disagreeing_shape_reports_resolved_and_reason() {
    let server = TestServer::start_with_kinds(KindRegistry::with_builtin()).await;
    let (_ns, key) = signup(&server.base_url, "Kind Honor AC3b").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let result = client
        .tools_call(
            "host.spec_test",
            json!({
                "kind": "echo",
                "spec": {
                    "schema": {"type": "object"},
                    "source": "def main(args):\n    return {}\n",
                },
                "invocations": [],
            }),
        )
        .await
        .expect("host.spec_test still runs -- echo's own validate ignores the stray field");
    let structured = common::extract_structured(&result);
    assert_eq!(structured["kind"]["resolved"], json!("python"));
    assert_eq!(structured["kind"]["requested"], json!("echo"));
    let reason = structured["kind"]["reason"]
        .as_str()
        .expect("reason present on disagreement");
    assert!(reason.contains("source"), "reason: {reason}");
}
