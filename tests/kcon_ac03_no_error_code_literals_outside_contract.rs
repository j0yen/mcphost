//! AC3 — Given `src/kinds/`, When
//! `grep -rn '"tool_\|"upstream_\|"compose_\|"host_not_allowed"' src/kinds/ | grep -v contract.rs`
//! runs, Then it prints nothing.

use std::process::Command;

#[test]
fn kcon_ac03_no_error_code_literals_outside_contract() {
    let root = env!("CARGO_MANIFEST_DIR");
    let out = Command::new("sh")
        .current_dir(root)
        .arg("-c")
        .arg(r#"grep -rn '"tool_\|"upstream_\|"compose_\|"host_not_allowed"' src/kinds/ | grep -v contract.rs"#)
        .output()
        .expect("sh and grep run");
    let printed = String::from_utf8_lossy(&out.stdout);
    assert!(
        printed.trim().is_empty(),
        "error-code literals outside src/kinds/contract.rs:\n{printed}"
    );
}

/// The grep above is vacuous if `contract.rs` is where the literals live
/// and no kind refers to them -- prove the table is the one definition.
#[test]
fn kcon_ac03_contract_rs_spells_every_code_once() {
    let root = env!("CARGO_MANIFEST_DIR");
    let src = std::fs::read_to_string(format!("{root}/src/kinds/contract.rs")).expect("contract.rs");
    for code in ["tool_exception", "tool_timeout", "upstream_error", "compose_self_call", "host_not_allowed"] {
        assert_eq!(
            src.matches(&format!("= \"{code}\";")).count(),
            1,
            "{code} is defined exactly once"
        );
    }
}
