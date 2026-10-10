//! PRD-mcphost-contract-version-reported AC6 — Given
//! `contracts/host-tools.v1.json` on disk at the PR head, When the drift
//! test hashes it, Then it equals `contract_sha(&KindRegistry::with_builtin())`;
//! Given one description edited without `mcphost contract dump`, Then the
//! test fails and its message contains `mcphost contract dump`.

use mcphost::api_contract::{contract_sha, sha_hex};
use mcphost::kinds::KindRegistry;

fn drift_check(file_bytes: &[u8]) -> Result<(), String> {
    let on_disk = sha_hex(file_bytes);
    let served = contract_sha(&KindRegistry::with_builtin());
    if on_disk == served {
        Ok(())
    } else {
        Err(format!(
            "contracts/host-tools.v1.json sha {on_disk} != served contract_sha {served}; \
             regenerate it with `mcphost contract dump` and commit the result"
        ))
    }
}

fn committed() -> Vec<u8> {
    std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/contracts/host-tools.v1.json"))
        .expect("read contracts/host-tools.v1.json")
}

#[test]
fn committed_contract_file_hashes_to_the_served_contract_sha() {
    if let Err(msg) = drift_check(&committed()) {
        panic!("{msg}");
    }
}

#[test]
fn an_edited_description_fails_the_drift_check_naming_the_fix() {
    let text = String::from_utf8(committed()).expect("utf-8");
    let marker = "\"description\": \"";
    let at = text.find(marker).expect("a description") + marker.len();
    let mut edited = text.clone();
    edited.insert_str(at, "EDITED ");
    let err = drift_check(edited.as_bytes()).expect_err("an edit must be detected");
    assert!(err.contains("mcphost contract dump"), "{err}");
}
