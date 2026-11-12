//! PRD-mcphost-pii-tagging-and-log-redaction-v2
//! AC4 -- Given an http tool whose upstream echoes the request in a 500 body,
//! When called with a tagged value, plain and percent-encoded in the body, Then
//! the call's `logs` line, `call_traces.error_json`, `calls.error_summary` and
//! (async) `runs.error_data_json` show `[redacted:pii]` and hold neither form.

use crate::ctrace;
use crate::piir;

use mcphost::logsink::REDACTION;
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const EMAIL: &str = "a@b.io";
const ENCODED: &str = "a%40b.io";

/// A 500 whose body repeats the request body and the same value percent-encoded.
async fn echoing_upstream() -> MockServer {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(|req: &Request| {
            let body = String::from_utf8_lossy(&req.body).to_string();
            ResponseTemplate::new(500).set_body_string(format!("upstream failed: request={body} query=email%3D{ENCODED}"))
        })
        .mount(&upstream)
        .await;
    upstream
}

#[tokio::test]
async fn piir_ac04_http_upstream_echo_redacted() {
    let _switch = ctrace::trace_switch().await;
    let upstream = echoing_upstream().await;
    let url = format!("{}/echo", upstream.uri());

    // Baseline: untagged, the sinks hold both forms.
    let base = piir::start_http().await;
    base.publish_http("echo", &url, None).await.expect("publish untagged");
    base.call("echo", json!({"email": EMAIL})).await.expect_err("the upstream answers 500");
    let held = base.sinks();
    println!("piir_ac04: untagged baseline: plain in {:?}, encoded in {:?}", held.holding(EMAIL), held.holding(ENCODED));
    assert!(held.holding(EMAIL).contains(&"ToolLog"), "premise: the logs line holds the value: {held:#?}");

    let fx = piir::start_http().await;
    fx.publish_http("echo", &url, Some(json!(["email"]))).await.expect("publish tagged");
    let err = fx.call("echo", json!({"email": EMAIL})).await.expect_err("the upstream answers 500");
    assert!(err.data.to_string().contains(EMAIL), "the caller still gets the upstream excerpt: {err:?}");
    let run = fx.run_async("echo", json!({"email": EMAIL})).await;
    assert_eq!(run["status"], json!("error"), "{run}");

    let sinks = fx.sinks();
    for form in [EMAIL, ENCODED] {
        assert!(sinks.holding(form).is_empty(), "no sink holds {form}: {sinks:#?}");
    }
    // The call's log line, its trace's error, the audit summary and the run's
    // error data show the marker where the untagged call held a form.
    for sink in ["ToolLog", "CallTrace", "CallErrorSummary", "RunError"] {
        let was_held = [EMAIL, ENCODED].iter().any(|f| held.holding(f).contains(&sink));
        if was_held {
            // A trace stores the tagged request key masked (`***`), as a secret-looking key is.
            let shows = if sink == "CallTrace" { "\"email\":\"***\"" } else { REDACTION };
            assert!(sinks.text_of(sink).contains(shows), "{sink} shows {shows}: {sinks:#?}");
        }
    }
    let log = sinks.text_of("ToolLog");
    assert!(log.contains(&format!("request={{\"email\":\"{REDACTION}\"}}")), "the log line: {log}");
    assert!(log.contains(&format!("email%3D{REDACTION}")), "the percent-encoded form is replaced too: {log}");
    // The async run's error data is redacted as well.
    assert!(sinks.text_of("RunError").contains(REDACTION), "runs.error_data_json: {sinks:#?}");
}
