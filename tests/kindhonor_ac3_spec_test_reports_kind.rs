//! AC3 -- Given `host.spec_test` on any spec, When it runs, Then the report
//! includes `kind.resolved` and, if a kind was requested, `kind.requested`
//! with a `reason` on disagreement.
//!
//! Uses the `echo` kind alone (no sandbox/network dependency): its own
//! `validate` only requires `spec.schema`, so a spec that *also* carries an
//! unrelated `source` field (python's own required field) used to validate
//! successfully under `echo` while structurally implying `python` --
//! PRD-mcphost-spec-unknown-field-rejection closed exactly that leniency
//! (`echo`'s own `known_spec_fields()` is `["schema"]` alone), so the same
//! fixture now proves the newer, stricter check wins the race: a spec
//! carrying a field `echo` doesn't understand fails `unknown_spec_field`
//! before `host.spec_test` ever reaches this kind-disagreement report (a
//! real `http`-vs-`python` disagreement always failed each kind's own
//! required-field validation first anyway, the same as `host.tool_publish`,
//! and never reached this report either).

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
async fn a_structurally_disagreeing_field_now_fails_unknown_spec_field_first() {
    let server = TestServer::start_with_kinds(KindRegistry::with_builtin()).await;
    let (_ns, key) = signup(&server.base_url, "Kind Honor AC3b").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    // PRD-mcphost-spec-unknown-field-rejection: `source` is not one of
    // echo's own known_spec_fields() (`["schema"]`) -- this now fails
    // before host.spec_test ever reaches the kind-disagreement report this
    // AC's success path used to exercise with this exact fixture.
    let err = client
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
        .expect_err("echo no longer silently ignores a field it doesn't understand");
    assert_eq!(err.error_code.as_deref(), Some("unknown_spec_field"));
    assert_eq!(err.data.get("field"), Some(&json!("source")));
}
