//! PRD-mcphost-python-sdk-generated AC6 (P0) -- Given a response missing a
//! required result field, When parsed, Then `ResultShapeError` names the tool
//! and field; Given a response with an extra field, Then parsing succeeds and
//! the field is reachable via `model_extra`.

use crate::pysdk;

#[test]
fn pysdk_ac06_missing_field_names_tool_and_field_and_extra_field_is_kept() {
    pysdk::pytest_ok("tests/test_result_shape.py");
}
