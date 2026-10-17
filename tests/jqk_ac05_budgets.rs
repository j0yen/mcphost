//! PRD-mcphost-jq-kind
//! AC5 -- Given `program: "repeat(.)"` and `timeout_s: 1`, When called, Then
//! `tool_timeout` within 2 s and a second call to another tool succeeds;
//! Given output over `max_output_bytes`, Then `tool_output_too_large`.

use mcphost::kinds::echo::EchoKind;
use mcphost::kinds::jq::JqKind;
use mcphost::kinds::{CallCtx, Kind, KindError};
use serde_json::json;

#[tokio::test]
async fn jqk_ac05_a_runaway_program_times_out_and_the_host_serves_the_next_call() {
    let spec = json!({"program": "repeat(.)", "timeout_s": 1});
    let ctx = CallCtx::for_test(1, "t_jqk_ac05");
    let err = crate::deadline_budget!(2000, {
        JqKind.call(&spec, json!({}), &ctx).await.expect_err("a runaway program is stopped")
    });
    let KindError::Structured { code, data, .. } = err else { panic!("structured") };
    assert_eq!(code, "tool_timeout");
    assert_eq!(data["families"], json!(["tool_limit"]));

    // The host serves the next call: another tool, and the jq kind itself.
    let echo = EchoKind.call(&json!({}), json!({"msg": "next"}), &ctx).await.expect("echo still answers");
    assert_eq!(echo, json!({"msg": "next"}));
    let again = JqKind.call(&json!({"program": ".n + 1"}), json!({"n": 1}), &ctx).await.expect("jq still answers");
    assert_eq!(again, json!(2));
}

#[tokio::test]
async fn jqk_ac05_a_term_that_never_yields_still_times_out_on_time() {
    // A single term that yields nothing until it ends (`reduce` here, finite
    // so the abandoned thread is not left spinning for the whole test run)
    // cannot check its deadline: the call answers on time and the evaluator
    // thread is abandoned and counted.
    let spec = json!({"program": "reduce range(0; 20000000) as $i (0; . + $i)", "timeout_s": 1});
    let ctx = CallCtx::for_test(1, "t_jqk_ac05");
    let before = mcphost::kinds::jq::abandoned_evaluators();
    let err = crate::deadline_budget!(2000, {
        JqKind.call(&spec, json!({}), &ctx).await.expect_err("stopped")
    });
    let KindError::Structured { code, .. } = err else { panic!("structured") };
    assert_eq!(code, "tool_timeout");
    assert!(mcphost::kinds::jq::abandoned_evaluators() > before, "the abandoned evaluator is counted");
}

#[tokio::test]
async fn jqk_ac05_output_over_the_cap_is_tool_output_too_large() {
    let spec = json!({"program": "[range(0; 100000)]", "max_output_bytes": 1024});
    let ctx = CallCtx::for_test(1, "t_jqk_ac05");
    let err = JqKind.call(&spec, json!({}), &ctx).await.expect_err("the output is over the cap");
    let KindError::Structured { code, message, data } = err else { panic!("structured") };
    assert_eq!(code, "tool_output_too_large");
    assert_eq!(data["max_output_bytes"], json!(1024));
    assert_eq!(data["families"], json!(["tool_limit"]));
    assert!(message.contains("1024"), "the message names the cap: {message}");

    // Several outputs also count against the cap, not only a single value.
    let many = json!({"program": "range(0; 5000)", "max_output_bytes": 1024});
    let err = JqKind.call(&many, json!({}), &ctx).await.expect_err("over the cap");
    let KindError::Structured { code, .. } = err else { panic!("structured") };
    assert_eq!(code, "tool_output_too_large");
}
