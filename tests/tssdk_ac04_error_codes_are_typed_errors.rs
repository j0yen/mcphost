//! PRD-mcphost-typescript-sdk-generated AC4 (P0) -- Given a test host
//! returning `args_invalid` with `data.argument = "spec"`, When received,
//! Then `ArgsInvalid` is thrown with `.code`, `.data.argument` and
//! `instanceof MCPHostError`; an unknown code throws `MCPHostError` with the
//! code preserved.

use crate::tssdk;

#[test]
fn tssdk_ac04_args_invalid_is_a_typed_error_and_unknown_codes_keep_their_code() {
    tssdk::vitest_ok("tests/errors.test.ts");
}
