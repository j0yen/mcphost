//! PRD-mcphost-call-trace-replay-b (ctraceb)
//! AC3 -- Given a failed trace, When replayed with `replay: {"args":
//! {"table": "users"}}`, Then the response carries `replay.result`,
//! `replay.verdict`, `replay.diff.ok: [false, true]`, `delivered: false`, and
//! the original trace unchanged; Given a replay that leaves a `***` value in
//! place, Then `replay_needs_secret` names the key and nothing runs.

use crate::ctrace;
use crate::common;

use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn ctraceb_replay_reruns_the_tool_as_a_dry_run_and_refuses_a_value_still_masked() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let _switch = ctrace::trace_switch().await;
    let fx = ctrace::start().await;
    let tenant_id = fx.tenant_id().await;
    fx.publish_python_spec(
        "dump",
        json!({
            "source": "def main(args):\n    if args['table'] != 'users':\n        raise KeyError(args['table'])\n    return {'rows': 3}\n",
            "args_schema": {"type": "object"},
        }),
    )
    .await;
    fx.warm("dump", json!({"table": "users"})).await;

    // A failed call, then its replay with one argument changed.
    let err = fx.tool_call("dump", json!({"table": "orders"})).await.expect_err("orders is not a table");
    let call_id = err.data["call_id"].as_str().expect("call_id in the error data").to_string();
    let before = fx.trace(&fx.client, &call_id).await.expect("the failed call's trace");
    assert_eq!(before["outcome"], json!("error"), "{before}");

    let replayed = fx
        .client
        .tools_call("host.call_trace", json!({"call_id": call_id, "replay": {"args": {"table": "users"}}}))
        .await
        .map(|v| common::extract_structured(&v))
        .unwrap_or_else(|e| panic!("replay: {} {}", e.code, e.message));
    assert_eq!(replayed["delivered"], json!(false), "{replayed}");
    assert_eq!(replayed["replay"]["diff"]["ok"], json!([false, true]), "{replayed}");
    assert_eq!(replayed["replay"]["verdict"], json!("pass"), "{replayed}");
    assert_eq!(replayed["replay"]["result"]["result"]["rows"], json!(3), "the dry-run result of the same tool: {replayed}");
    assert_eq!(replayed["replay"]["result"]["dry_run"]["delivered"], json!(false), "{replayed}");
    assert_eq!(replayed["replay"]["diff"]["error_code"], json!(["tool_exception", null]), "{replayed}");
    // The original, as it sits in the response, is the trace read before.
    for key in ["call_id", "tool", "outcome", "request", "stdout_tail", "stderr_tail", "line", "source_line", "error_code"] {
        assert_eq!(replayed[key], before[key], "{key} of the original trace is untouched: {replayed}");
    }
    assert_eq!(fx.trace(&fx.client, &call_id).await.expect("re-read"), before, "the stored trace is unchanged");

    // A call that carried a secret-looking argument: its trace holds `***`.
    let err = fx
        .tool_call("dump", json!({"table": "orders", "api_key": "sk-live-9"}))
        .await
        .expect_err("fails again");
    let masked_id = err.data["call_id"].as_str().expect("call_id").to_string();
    let rows_before = fx.server.state.db.latest_call_id_for_test(tenant_id).await.expect("latest");
    let refused = fx
        .client
        .tools_call("host.call_trace", json!({"call_id": masked_id, "replay": {"args": {"table": "users"}}}))
        .await
        .expect_err("a masked value cannot be replayed");
    assert_eq!(refused.error_code.as_deref(), Some("replay_needs_secret"), "{refused:?}");
    assert_eq!(refused.data["key"], json!("api_key"), "the error names the key: {refused:?}");
    assert!(refused.message.contains("api_key"), "{refused:?}");
    let rows_after = fx.server.state.db.latest_call_id_for_test(tenant_id).await.expect("latest");
    assert_eq!(rows_before, rows_after, "nothing ran: no call row was written");

    // Supplying the value in replay.args makes the same trace replayable.
    let ok = fx
        .client
        .tools_call(
            "host.call_trace",
            json!({"call_id": masked_id, "replay": {"args": {"table": "users", "api_key": "sk-live-9"}}}),
        )
        .await
        .map(|v| common::extract_structured(&v))
        .unwrap_or_else(|e| panic!("replay with the secret: {} {}", e.code, e.message));
    assert_eq!(ok["replay"]["diff"]["ok"], json!([false, true]), "{ok}");
    // ... and the raw value still is not stored anywhere.
    let stored = fx.server.state.db.call_traces_dump_for_test().await.expect("dump");
    assert!(!stored.contains("sk-live-9"), "{stored}");
}
