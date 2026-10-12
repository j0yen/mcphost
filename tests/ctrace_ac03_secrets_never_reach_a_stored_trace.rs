//! PRD-mcphost-call-trace-replay
//! AC3 -- Given a tool called with `{"api_key": "sk-live-1", "q": "<the value
//! of secret S>", "n": 3}`, When its trace is stored, Then `request.args` is
//! `{"api_key": "***", "q": "{{ secret.S }}", "n": 3}` and neither raw value
//! appears anywhere in `call_traces`.

use crate::ctrace;

use mcphost::sandbox;
use serde_json::json;

const SECRET_VALUE: &str = "hunter2-the-secret-value";

#[tokio::test]
async fn masked_key_and_secret_value_never_appear_in_call_traces() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let _switch = ctrace::trace_switch().await;
    let fx = ctrace::start().await;
    fx.client
        .tools_call("host.secret_set", json!({"name": "S", "value": SECRET_VALUE}))
        .await
        .expect("host.secret_set");
    // The tool echoes both values to stdout and stderr: the output tails are
    // part of "anywhere in call_traces".
    fx.publish_python_spec(
        "lookup",
        json!({
            "source": "import sys\ndef main(args):\n    print('key=' + args['api_key'] + ' q=' + args['q'])\n    print('err ' + args['q'], file=sys.stderr)\n    return {'n': args['n']}\n",
            "args_schema": {"type": "object"},
        }),
    )
    .await;
    fx.warm("lookup", json!({"api_key": "x", "q": "y", "n": 0})).await;

    let args = json!({"api_key": "sk-live-1", "q": SECRET_VALUE, "n": 3});
    let result = fx.tool_call("lookup", args).await.expect("host.tool_call");
    let call_id = result["call_id"].as_str().expect("call_id");

    let trace = fx.trace(&fx.client, call_id).await.expect("host.call_trace");
    assert_eq!(
        trace["request"]["args"],
        json!({"api_key": "***", "q": "{{ secret.S }}", "n": 3}),
        "the stored request: {trace}"
    );
    assert_eq!(trace["request"]["tool"], json!("lookup"), "{trace}");
    // Output proves the tool really saw the raw values, so the tails below
    // were scrubbed rather than empty.
    let stdout = trace["stdout_tail"].as_str().expect("stdout_tail");
    assert!(stdout.contains("key=***") && stdout.contains("q={{ secret.S }}"), "stdout_tail: {stdout:?}");

    let stored = fx.server.state.db.call_traces_dump_for_test().await.expect("dump call_traces");
    assert!(!stored.is_empty(), "a trace row was written");
    for raw in ["sk-live-1", SECRET_VALUE] {
        assert!(!stored.contains(raw), "{raw} must not appear anywhere in call_traces: {stored}");
    }
}
