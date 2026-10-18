//! PRD-mcphost-typescript-sdk-generated AC5 (P0) -- Given a `/contract.json`
//! ETag that differs from `CONTRACT_SHA`, When the client is constructed,
//! Then exactly one `console.warn` names both versions; with
//! `onSkew: "throw"` the constructor rejects with `ContractSkew` (a JS
//! constructor cannot be async: the rejection is `client.ready`, which every
//! call awaits).

use crate::tssdk;

#[test]
fn tssdk_ac05_skew_warns_once_naming_both_versions_and_rejects_when_asked_to_throw() {
    tssdk::vitest_ok("tests/skew.test.ts");
}
