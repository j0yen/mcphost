//! PRD-mcphost-dry-run-inside-savepoint-for-transactional-verbs
//! AC4 -- `tests/txn_allowlist.txt` is generated (`TXN_ALLOWLIST_WRITE=1`),
//! a new raw `BEGIN IMMEDIATE` outside it fails the lint naming file:line,
//! the writer refuses a list longer than `TXN_ALLOWLIST_CEILING`, and the
//! committed list equals the writer's output at the landed tree.

use crate::dryspt_lint;

use mcphost::txn::TXN_ALLOWLIST_CEILING;

fn root() -> &'static std::path::Path {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn dryspt_ac04() {
    let list_path = root().join("tests/txn_allowlist.txt");
    let findings = dryspt_lint::scan_src(root());

    if std::env::var("TXN_ALLOWLIST_WRITE").as_deref() == Ok("1") {
        let text = dryspt_lint::render_checked(&findings, TXN_ALLOWLIST_CEILING).unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(&list_path, text).expect("write allow-list");
    }
    let committed = std::fs::read_to_string(&list_path).expect("tests/txn_allowlist.txt is committed");

    // Subset: the landed tree has no site outside the list.
    dryspt_lint::check_subset(&findings, &committed).unwrap_or_else(|e| panic!("{e}"));

    // Fresh: the committed list is exactly what the writer would produce.
    assert_eq!(
        committed,
        dryspt_lint::render_allowlist(&findings),
        "tests/txn_allowlist.txt is stale: run TXN_ALLOWLIST_WRITE=1 cargo test dryspt_ac04"
    );

    // Only shrinks: no longer than the ceiling.
    let n = committed.lines().count();
    assert!(n <= TXN_ALLOWLIST_CEILING, "allow-list has {n} lines, ceiling is {TXN_ALLOWLIST_CEILING}");

    // A new raw BEGIN IMMEDIATE in a src/ function outside the list fails
    // the lint, naming that file:line and the helper.
    let mut with_new = findings.clone();
    with_new.extend(dryspt_lint::scan_text(
        "src/brand_new.rs",
        "fn fresh(conn: &Connection) {\n    conn.execute(\"BEGIN IMMEDIATE\", []);\n}\n",
    ));
    let err = dryspt_lint::check_subset(&with_new, &committed).expect_err("a new raw site must fail the lint");
    assert!(err.contains("src/brand_new.rs:2"), "failure names file:line: {err}");
    assert!(err.contains("src/txn.rs::with_tx"), "failure points at the helper: {err}");

    // The writer refuses a list longer than the ceiling.
    let refused = dryspt_lint::render_checked(&with_new, TXN_ALLOWLIST_CEILING);
    assert!(refused.is_err(), "writer must refuse {} lines over ceiling {TXN_ALLOWLIST_CEILING}", with_new.len());
    assert!(dryspt_lint::render_checked(&findings, TXN_ALLOWLIST_CEILING).is_ok());
}
