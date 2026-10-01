//! AC3 -- Given `host.spec_test` on any spec, When it runs, Then the report
//! includes `kind.resolved` and, if a kind was requested, `kind.requested`
//! with a `reason` on disagreement.
//!
//! Uses the `echo` kind alone (no sandbox/network dependency): its own
//! `validate` only requires `spec.schema`.
//!
//! `disagreeing_shape_reports_resolved_and_reason` below originally used a
//! spec carrying an unrelated `source` field (python's own field) to reach
//! this report with a genuine kind disagreement -- echo's pre-existing
//! `validate` silently ignored any key it didn't recognize, so that spec
//! validated successfully under `echo` despite structurally implying
//! `python`. PRD-mcphost-spec-unknown-field-rejection closed exactly that
//! leniency (every registered kind, echo included, now runs
//! `check_unknown_spec_fields` before anything else in `validate`/
//! `validate_all`): a spec with a field `echo` doesn't recognize no longer
//! reaches this report at all -- it fails `unknown_spec_field` first, same
//! as `host.tool_publish` would. That is a *better* answer to "this spec
//! looks like a different kind" (`unknown_spec_field`'s own
//! `data.valid_for` names `python` directly, from the literal field, no
//! shape-inference heuristic needed) for the one case `infer_kind_signal`
//! could reach through a successful validate; that case no longer exists
//! for ANY kind, so this test now asserts the `unknown_spec_field` refusal
//! instead of a disagreement report.

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

/// PRD-mcphost-spec-unknown-field-rejection superseded this test's
/// original premise (see the module doc above): a spec shape implying a
/// different kind, via a field that kind doesn't recognize, is now refused
/// as `unknown_spec_field` -- with `data.valid_for` naming the kind the
/// field actually belongs to -- before `host.spec_test` ever reaches the
/// `kind.resolved`/`kind.requested`/`reason` disagreement report this test
/// used to exercise.
#[tokio::test]
async fn disagreeing_shape_is_refused_as_unknown_spec_field_before_any_disagreement_report() {
    let server = TestServer::start_with_kinds(KindRegistry::with_builtin()).await;
    let (_ns, key) = signup(&server.base_url, "Kind Honor AC3b").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

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
        .expect_err("echo no longer tolerates an unrecognized field, even a python-shaped one");

    assert_eq!(err.error_code.as_deref(), Some("unknown_spec_field"));
    assert_eq!(err.data["field"], json!("source"));
    assert_eq!(err.data["kind"], json!("echo"));
}
