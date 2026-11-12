//! PRD-mcphost-pii-tagging-and-log-redaction-v2
//! AC1 -- Given a python tool published without `pii` that prints its `email`
//! argument and raises on it, When called with `email: "a@b.io"`, Then
//! `call_traces` (request, output tails, error) and `calls.error_summary` hold
//! `a@b.io` and the test prints which; if no persisted sink holds it the build
//! stops with `premise_false`.

use crate::ctrace;
use crate::piir;

use mcphost::sandbox;
use serde_json::json;

const EMAIL: &str = "a@b.io";

#[tokio::test]
async fn piir_ac01_untagged_value_persists_in_sinks() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let _switch = ctrace::trace_switch().await;
    let fx = piir::start().await;
    fx.publish("leaky", piir::LEAKY_SOURCE, None).await.expect("publish without pii");
    fx.warm("leaky").await;

    let err = fx.call("leaky", json!({"email": EMAIL})).await.expect_err("the tool raises");
    assert!(err.message.contains(EMAIL), "the caller-facing error carries the value, as the premise probe saw: {}", err.message);

    let sinks = fx.sinks();
    let held = sinks.holding(EMAIL);
    println!("piir_ac01: sinks holding {EMAIL} for an untagged tool: {held:?}");
    // The premise: with no tag, some persisted sink holds the value. None
    // holding it would mean there is nothing for this PRD to redact.
    assert!(!held.is_empty(), "premise_false: no persisted sink holds {EMAIL}");
    for expected in ["CallTrace", "CallErrorSummary"] {
        assert!(held.contains(&expected), "{expected} holds {EMAIL}: held={held:?}");
    }
    // The pieces of the trace the PRD names.
    let trace = sinks.text_of("CallTrace");
    assert!(trace.contains(&format!("\"email\":\"{EMAIL}\"")), "request holds it: {trace}");
    assert!(trace.contains(&format!("email={EMAIL}")), "stdout tail holds it: {trace}");
    assert!(trace.contains(&format!("err {EMAIL}")), "stderr tail holds it: {trace}");
    assert!(trace.contains(&format!("bad email {EMAIL}")), "error holds it: {trace}");
}
