//! PRD-mcphost-jq-kind
//! AC7 -- Given `program: "input"`, When published, Then
//! `jq_builtin_unsupported` names `input` and lists the allowed set, and that
//! list equals the kind's registered definitions minus the exclusions.

use mcphost::errors::AppError;
use mcphost::kinds::jq::{EXCLUDED_BUILTINS, JqKind, allowed_builtins, registered_builtins};
use mcphost::kinds::{Kind, KindError};
use serde_json::{Value, json};

fn refusal(program: &str) -> (String, Value) {
    let err = JqKind.validate(&json!({"program": program})).expect_err("an unsupported builtin is refused");
    let KindError::Structured { code, message, data } = err else {
        panic!("structured, got a different error for `{program}`");
    };
    assert_eq!(code, "jq_builtin_unsupported", "`{program}`: {message}");
    (message, data)
}

#[test]
fn jqk_ac07_input_is_refused_by_name_with_the_allowed_set() {
    let (message, data) = refusal("input");
    assert_eq!(data["builtin"], json!("input"));
    assert!(message.contains("`input`"), "the message names it: {message}");

    let allowed: Vec<String> = serde_json::from_value(data["allowed"].clone()).expect("allowed is a list of strings");
    assert!(!allowed.is_empty());
    assert!(allowed.iter().all(|a| message.contains(a.as_str())), "the message lists the allowed set");

    // The listed set is the registered definitions minus the exclusions.
    let expected: Vec<String> = registered_builtins()
        .into_iter()
        .filter(|b| !EXCLUDED_BUILTINS.contains(&b.rsplit_once('/').map_or(b.as_str(), |(name, _)| name)))
        .collect();
    assert_eq!(allowed, expected);
    assert_eq!(allowed, allowed_builtins());
    for excluded in EXCLUDED_BUILTINS {
        assert!(
            !allowed.iter().any(|a| a.rsplit_once('/').map_or(a.as_str(), |(name, _)| name) == *excluded),
            "`{excluded}` must not be listed as allowed"
        );
    }
    assert!(allowed.iter().any(|a| a == "map/1") && allowed.iter().any(|a| a == "select/1"));
}

#[test]
fn jqk_ac07_every_excluded_builtin_is_refused_at_publish() {
    for (program, name) in [
        ("[inputs]", "inputs"),
        ("env", "env"),
        ("$ENV", "$ENV"),
        ("@sh", "@sh"),
        ("halt", "halt"),
        ("import \"a\" as a; .", "import"),
    ] {
        let (_, data) = refusal(program);
        assert_eq!(data["builtin"], json!(name), "`{program}`");
    }
}

#[test]
fn jqk_ac07_the_refusal_is_a_caller_input_error_on_the_wire() {
    let err = JqKind.validate(&json!({"program": "input"})).expect_err("refused");
    let wire = AppError::from(err).into_error_data();
    assert_eq!(wire.code.0, -32602, "invalid params, not an internal error");
    assert_eq!(wire.data.expect("data")["error_code"], json!("jq_builtin_unsupported"));
}

#[test]
fn jqk_ac07_an_unknown_name_is_not_reported_as_an_unsupported_builtin() {
    let err = JqKind.validate(&json!({"program": "nosuchfilter"})).expect_err("undefined");
    let KindError::Structured { code, data, .. } = err else { panic!("structured") };
    assert_eq!(code, "invalid_spec");
    assert_eq!(data["line"], json!(1));
    assert_eq!(data["column"], json!(1));
}
