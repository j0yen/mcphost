//! PRD-mcphost-heal-action-registry-and-ledger AC4 (P0): no `HealAction`
//! variant is destructive, and the executor module carries no statement that
//! could remove rows or tables.

use mcphost::heal::HealAction;

#[test]
fn heal_ac_no_destructive_variant() {
    for action in HealAction::ALL {
        assert!(!action.policy().destructive, "{} must not be destructive", action.as_str());
    }
}

#[test]
fn executor_module_has_no_delete_or_drop() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/heal.rs")).expect("read heal.rs");
    for (n, line) in src.lines().enumerate() {
        let upper = line.to_uppercase();
        for banned in ["DELETE", "DROP", "TRUNCATE"] {
            assert!(
                !upper.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| w == banned),
                "src/heal.rs:{} mentions {banned}: {line}",
                n + 1
            );
        }
    }
    // The module writes through the ledger methods only, never raw SQL.
    assert!(!src.contains("with_conn"), "executor runs no SQL of its own");
}
