//! PRD-mcphost-dry-run-inside-savepoint-for-transactional-verbs
//! AC3 -- the raw-transaction lint reports every planted pattern (including
//! the two after `"https://x"` and `"/*"` literals), reports neither comment,
//! and finds nothing in the 8 migrated functions at the landed tree.

use crate::dryspt_lint;

const FIXTURE: &str = r####"fn a(conn: &Connection) {
    conn.execute("BEGIN IMMEDIATE", []);
    conn.execute("COMMIT", []);
    conn.execute("ROLLBACK", []);
    conn.execute("SAVEPOINT sp", []);
    conn.execute("RELEASE sp", []);
    conn.execute("END TRANSACTION", []);
    conn.execute_batch("PRAGMA x; BEGIN; SELECT 1;");
    conn.unchecked_transaction();
    conn.transaction();
    transaction_with_behavior(x);
    conn.savepoint();
    savepoint_with_name(x);
    // conn.execute("BEGIN", []); conn.unchecked_transaction();
    /// conn.execute("COMMIT", []);
    let u = "https://x"; conn.execute("BEGIN IMMEDIATE", []);
    let g = "/*";
    conn.execute("ROLLBACK", []);
}
"####;

#[test]
fn dryspt_ac03() {
    let found = dryspt_lint::scan_text("fixture.rs", FIXTURE);
    let mut lines: Vec<usize> = found.iter().map(|f| f.line).collect();
    lines.sort();
    let expected: Vec<usize> = vec![2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 16, 18];
    assert_eq!(lines, expected, "every pattern (incl. after the \"https://x\" and \"/*\" literals) is reported: {found:#?}");
    assert!(found.iter().all(|f| f.line != 14 && f.line != 15), "comments are not reported");
    assert!(found.iter().all(|f| f.func == "a"));
    let tokens: Vec<&str> = found.iter().map(|f| f.token.as_str()).collect();
    for t in ["BEGIN", "COMMIT", "ROLLBACK", "SAVEPOINT", "RELEASE", "END TRANSACTION", "execute_batch(BEGIN)"] {
        assert!(tokens.contains(&t), "missing token {t}: {tokens:?}");
    }
    for t in dryspt_lint::CALLS {
        assert!(tokens.contains(&t), "missing call {t}: {tokens:?}");
    }

    // Raw strings and block comments hide nothing and invent nothing.
    let tricky = "fn b() {\n    let _s = r#\"//\"#; c.execute(\"BEGIN\", []);\n    /* c.execute(\"BEGIN\", []); /* nested */ c.unchecked_transaction(); */\n    let _q = '\"'; c.execute(\"COMMIT\", []);\n}\n";
    let t = dryspt_lint::scan_text("tricky.rs", tricky);
    assert_eq!(t.iter().map(|f| f.line).collect::<Vec<_>>(), vec![2, 4], "{t:#?}");

    // The landed tree: none of the 8 migrated functions holds a finding.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let all = dryspt_lint::scan_src(root);
    let migrated: [(&str, &str); 8] = [
        ("src/db.rs", "state_table_drop"),
        ("src/db.rs", "documents_put"),
        ("src/db.rs", "document_soft_delete"),
        ("src/index.rs", "index_create"),
        ("src/index.rs", "index_delete"),
        ("src/db.rs", "set_agent_profile"),
        ("src/db.rs", "purge_end_user"),
        ("src/db.rs", "revoke_all_oauth_grants_for_tenant"),
    ];
    for (path, func) in migrated {
        let hits: Vec<_> = all.iter().filter(|f| f.path == path && f.func == func).collect();
        assert!(hits.is_empty(), "{path} fn {func} still holds raw transaction SQL: {hits:#?}");
    }
    assert!(!all.is_empty(), "the 51 unmigrated sites are still found");
}
