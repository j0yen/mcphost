//! PRD-mcphost-example-from-published-tool
//! AC3 — Given a tool whose spec calls `host.state.insert`, When proposed,
//! Then `not_portable` names the reference; given a `proof_call` whose
//! `expect` does not hold now, Then `proof_failed` with the actual result
//! excerpt.

use crate::exprp;

use crate::common::chain_and_http_kind_registry;
use serde_json::json;

#[tokio::test]
async fn a_chain_step_on_the_state_store_is_not_portable_and_names_the_reference() {
    let f = exprp::start_with(chain_and_http_kind_registry()).await;
    f.client
        .tools_call(
            "host.tool_publish",
            json!({"name": "stateful", "kind": "chain", "spec": {"steps": [{"tool": "host.state.set", "args": {"key": "k", "value": "v"}}]}}),
        )
        .await
        .expect("publish chain");

    let err = exprp::propose(&f.client, exprp::propose_args("stateful")).await.expect_err("state use is not portable");
    assert_eq!(err.error_code.as_deref(), Some("not_portable"), "{} {}", err.code, err.message);
    assert_eq!(err.data["reference"], "host.state.set");
    assert!(err.message.contains("host.state.set"), "message names it: {}", err.message);
    assert!(f.server.state.db.list_example_proposals(None).await.expect("list").is_empty());
}

#[test]
fn the_detector_names_host_state_insert_and_other_tenants_tools() {
    let hit = mcphost::example_propose::find_non_portable("import mcphost\nmcphost.call('host.state.insert', {})", &json!({}), "t_me");
    assert_eq!(hit.as_deref(), Some("host.state.insert"));
    let theirs = mcphost::example_propose::find_non_portable("", &json!({"steps": [{"tool": "t_other.geo"}]}), "t_me");
    assert_eq!(theirs.as_deref(), Some("t_other.geo"));
    let own = mcphost::example_propose::find_non_portable("", &json!({"steps": [{"tool": "t_me.geo"}, {"tool": "host.tool_call"}]}), "t_me");
    assert_eq!(own, None);
}

#[tokio::test]
async fn a_proof_call_that_does_not_hold_now_returns_proof_failed_with_the_actual_excerpt() {
    let f = exprp::start().await;
    exprp::publish_http(&f.client, "probe", exprp::probe_spec(&f.upstream)).await;
    let mut args = exprp::propose_args("probe");
    args["proof_call"]["expect"] = json!("result.body.probed == 2");

    let err = exprp::propose(&f.client, args).await.expect_err("an expectation that fails now is refused");
    assert_eq!(err.error_code.as_deref(), Some("proof_failed"), "{} {}", err.code, err.message);
    let actual = err.data["actual"].as_str().expect("data.actual");
    assert!(actual.contains("probed"), "the excerpt shows the real result: {actual}");
    assert!(!format!("{} {}", err.message, err.data).contains(exprp::TOKEN_VALUE), "excerpt is scrubbed");
    assert!(f.server.state.db.list_example_proposals(None).await.expect("list").is_empty());
}
