//! PRD-mcphost-python-sdk-generated AC3 (P0) -- Given
//! `c.host.tool.publish(name="x", kind="nope", spec={})`, When called, Then a
//! `pydantic.ValidationError` is raised before any HTTP request (asserted
//! with a recording transport).

use crate::pysdk;

#[test]
fn pysdk_ac03_invalid_kind_raises_validation_error_before_any_request() {
    pysdk::pytest_ok("tests/test_publish_validation.py");
}
