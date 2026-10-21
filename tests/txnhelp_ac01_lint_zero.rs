//! PRD-mcphost-transactions-only-via-helper AC1 -- at landing the
//! predecessor's raw-transaction lint finds nothing in `src/` outside
//! `src/txn.rs` and `src/dryrun.rs`, `tests/txn_allowlist.txt` is empty and
//! equals what `TXN_ALLOWLIST_WRITE=1` would write, and the ceiling is 0.

use crate::dryspt_lint;

use mcphost::txn::TXN_ALLOWLIST_CEILING;

#[test]
fn txnhelp_ac01() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let findings = dryspt_lint::scan_src(root);
    assert!(findings.is_empty(), "raw transaction SQL left in src/ outside txn.rs/dryrun.rs: {findings:#?}");

    let committed = std::fs::read_to_string(root.join("tests/txn_allowlist.txt")).expect("allow-list is committed");
    assert!(committed.is_empty(), "tests/txn_allowlist.txt must be empty, found: {committed:?}");
    assert_eq!(committed, dryspt_lint::render_allowlist(&findings), "allow-list equals the writer's output");

    assert_eq!(TXN_ALLOWLIST_CEILING, 0, "the ceiling is 0 once the list is closed");
}
