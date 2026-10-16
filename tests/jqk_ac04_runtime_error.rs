//! PRD-mcphost-jq-kind
//! AC4 -- Given `program: ".a.b.c"` with `{"a": 1}`, When called, Then
//! `tool_exception` with `exception_class: jq_error`, a message naming
//! `Cannot index number with "b"`, `data.families: ["tool_code"]`.

use mcphost::kinds::jq::JqKind;
use mcphost::kinds::{CallCtx, Kind, KindError};
use serde_json::json;

#[tokio::test]
async fn jqk_ac04_a_runtime_error_is_a_tool_code_tool_exception() {
    let spec = json!({"program": ".a.b.c"});
    let ctx = CallCtx::for_test(1, "t_jqk_ac04");
    let err = JqKind.call(&spec, json!({"a": 1}), &ctx).await.expect_err("indexing a number fails");
    let KindError::Structured { code, message, data } = err else {
        panic!("a jq runtime error is structured");
    };
    assert_eq!(code, "tool_exception");
    assert_eq!(data["exception_class"], json!("jq_error"));
    assert_eq!(data["families"], json!(["tool_code"]));
    assert_eq!(data["phase"], json!("tool_code"));
    assert!(message.contains(r#"Cannot index number with "b""#), "message: {message}");
    assert!(data["message"].as_str().is_some_and(|m| m.contains(r#"Cannot index number with "b""#)), "{data}");
    // jaq errors carry no span: the whole program's first line, flagged unknown.
    assert_eq!(data["source_line"], json!(".a.b.c"));
    assert_eq!(data["position_known"], json!(false));
}
