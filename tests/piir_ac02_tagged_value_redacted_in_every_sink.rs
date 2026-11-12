//! PRD-mcphost-pii-tagging-and-log-redaction-v2
//! AC2 -- Given the same tool published with `pii: ["email"]`, When called, and
//! called again as an async run, Then every sink of `LogSink` shows
//! `[redacted:pii]` where the untagged call held `a@b.io`, none holds
//! `a@b.io`, the caller's `result` still contains it, and a `host.call_trace`
//! replay refuses naming `email`.

use crate::ctrace;
use crate::piir;

use mcphost::logsink::{LogSink, REDACTION};
use mcphost::sandbox;
use serde_json::json;

const EMAIL: &str = "a@b.io";

/// Prints the value and returns it: a tool whose `result` carries the tagged value.
const ECHO_SOURCE: &str = "import sys\ndef main(args):\n    print('email=' + args['email'])\n    print('err ' + args['email'], file=sys.stderr)\n    return {'echo': args['email']}\n";

#[tokio::test]
async fn piir_ac02_tagged_value_redacted_in_every_sink() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let _switch = ctrace::trace_switch().await;

    // The untagged baseline: which sinks hold the value without a tag.
    let base = piir::start().await;
    base.publish("leaky", piir::LEAKY_SOURCE, None).await.expect("publish untagged");
    base.warm("leaky").await;
    base.call("leaky", json!({"email": EMAIL})).await.expect_err("raises");
    let baseline = base.sinks().holding(EMAIL);
    assert!(!baseline.is_empty(), "premise: the untagged call persists the value somewhere");

    let fx = piir::start().await;
    let published = fx.publish("leaky", piir::LEAKY_SOURCE, Some(json!(["email"]))).await.expect("publish tagged");
    assert_eq!(published["name"], json!(format!("{}.leaky", fx.ns)), "{published}");
    fx.publish("echoer", ECHO_SOURCE, Some(json!(["email"]))).await.expect("publish tagged echoer");
    fx.warm("leaky").await;
    fx.warm("echoer").await;

    // A synchronous call: the caller's error still carries the value.
    let err = fx.call("leaky", json!({"email": EMAIL})).await.expect_err("raises");
    assert!(err.message.contains(EMAIL), "the caller-facing error is not redacted: {}", err.message);
    let call_id = err.data["call_id"].as_str().expect("call_id").to_string();
    // ... and so does a returned `result`.
    let echoed = fx.call("echoer", json!({"email": EMAIL})).await.expect("echoer returns");
    assert_eq!(echoed["echo"], json!(EMAIL), "the caller's result still contains the value: {echoed}");

    // An async run of the raising tool.
    let run = fx.run_async("leaky", json!({"email": EMAIL})).await;
    assert_eq!(run["status"], json!("error"), "{run}");

    let sinks = fx.sinks();
    println!("piir_ac02: untagged baseline held {baseline:?}; tagged holds {:?}", sinks.holding(EMAIL));
    assert!(sinks.holding(EMAIL).is_empty(), "no sink holds {EMAIL}: {sinks:#?}");
    for name in &baseline {
        assert!(sinks.text_of(name).contains(REDACTION), "{name} shows {REDACTION} where the untagged call held the value: {sinks:#?}");
    }
    // Every sink of the registry is one the test reads.
    let read: Vec<&str> = sinks.named().iter().map(|(n, _)| *n).collect();
    assert_eq!(read, LogSink::names(), "the fixture reads exactly the registry's sinks");
    // The async run's error data is a sink of its own.
    assert!(sinks.text_of("RunError").contains(REDACTION), "runs.error_data_json: {sinks:#?}");

    // The trace API shows the masked request and the marker in the tails.
    let trace = fx.call_trace(&call_id).await.expect("host.call_trace");
    assert_eq!(trace["request"]["args"]["email"], json!("***"), "{trace}");
    assert!(trace["stdout_tail"].as_str().unwrap_or("").contains(&format!("email={REDACTION}")), "{trace}");
    assert!(trace["stderr_tail"].as_str().unwrap_or("").contains(REDACTION), "{trace}");

    // A replay cannot restore the value, and says which argument.
    let refused = fx
        .client
        .tools_call("host.call_trace", json!({"call_id": call_id, "replay": {"args": {"note": "x"}}}))
        .await
        .expect_err("a tagged value cannot be replayed");
    assert_eq!(refused.error_code.as_deref(), Some("replay_needs_secret"), "{refused:?}");
    assert_eq!(refused.data["key"], json!("email"), "the refusal names the argument: {refused:?}");
}
