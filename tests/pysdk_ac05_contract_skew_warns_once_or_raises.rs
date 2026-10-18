//! PRD-mcphost-python-sdk-generated AC5 (P0) -- Given a test host whose
//! `/contract.json` ETag differs from `CONTRACT_SHA`, When `Client(url)` is
//! created, Then exactly one `ContractSkewWarning` is emitted naming both
//! versions; with `on_skew="raise"` the constructor raises `ContractSkew`.

use crate::pysdk;

#[test]
fn pysdk_ac05_skew_warns_once_naming_both_versions_and_raises_on_request() {
    pysdk::pytest_ok("tests/test_skew.py");
}
