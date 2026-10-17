//! PRD-mcphost-jq-kind
//! AC2 -- Given `program: "[.a | select(.x > 1]"`, When published, Then
//! `InvalidSpec` carries `line: 1`, `column: 20`, and `source_line` equal to
//! the program.

use mcphost::errors::AppError;
use mcphost::kinds::jq::JqKind;
use mcphost::kinds::{Kind, KindError};
use serde_json::json;

const PROGRAM: &str = "[.a | select(.x > 1]";

#[test]
fn jqk_ac02_unbalanced_bracket_is_refused_with_its_position() {
    let err = JqKind.validate(&json!({"program": PROGRAM})).expect_err("a bad program is refused");
    let KindError::Structured { code, data, .. } = &err else {
        panic!("a parse error is structured, got {err:?}");
    };
    assert_eq!(*code, "invalid_spec");
    assert_eq!(data["line"], json!(1));
    assert_eq!(data["column"], json!(20));
    assert_eq!(data["source_line"], json!(PROGRAM));

    // The same fields reach the wire through the host's own error conversion.
    let wire = AppError::from(err).into_error_data();
    let wire_data = wire.data.expect("the wire error carries data");
    assert_eq!(wire_data["error_code"], json!("invalid_spec"));
    assert_eq!(wire_data["line"], json!(1));
    assert_eq!(wire_data["column"], json!(20));
    assert_eq!(wire_data["source_line"], json!(PROGRAM));
}

#[test]
fn jqk_ac02_position_counts_lines_and_columns_on_a_multiline_program() {
    let program = ".a\n| select(.x > 1]";
    let err = JqKind.validate(&json!({"program": program})).expect_err("refused");
    let KindError::Structured { data, .. } = err else { panic!("structured") };
    assert_eq!(data["line"], json!(2));
    assert_eq!(data["column"], json!(16));
    assert_eq!(data["source_line"], json!("| select(.x > 1]"));
}
