//! PRD-mcphost-typescript-sdk-generated AC3 (P0) -- Given
//! `c.host.tool.publish({ name: "x", kind: "nope", spec: {} })`, When
//! compiled, Then `tsc` rejects `kind`; Given the same value cast to `any`,
//! When called, Then a `ZodError` is thrown before any `fetch` (asserted with
//! a mocked `fetch` that records zero calls).

use crate::tssdk;

#[test]
fn tssdk_ac03_invalid_kind_is_a_compile_error_and_a_zod_error_before_any_fetch() {
    // the vitest file carries a `@ts-expect-error` on the bad call: tsc fails
    // the build if the bad `kind` were accepted (unused directive), and fails
    // it if the directive were removed (type error)
    let test = std::fs::read_to_string(tssdk::sdk_dir().join("tests/publish-validation.test.ts")).unwrap();
    assert!(test.contains("@ts-expect-error"), "the compile-time half of AC3 is gone");
    let tsc = tssdk::npm(&["exec", "--", "tsc", "--strict", "--noEmit"]);
    assert!(tsc.status.success(), "tsc --strict --noEmit failed:\n{}", tssdk::text(&tsc));
    tssdk::vitest_ok("tests/publish-validation.test.ts");
}
