//! PRD-mcphost-export-data-class-completeness-v2
//! AC2 -- Given `EXPORT_EXCLUSIONS` as shipped and the covered set from
//! `ArchiveEntry::source_tables`, When the schema walk runs, Then the
//! uncovered list is empty; Given a test copy that adds table
//! `foo(tenant_id)`, Then the test fails naming `foo`.

use std::collections::BTreeSet;
use std::panic::{AssertUnwindSafe, catch_unwind};

use mcphost::db::Db;
use mcphost::export::{EXPORT_EXCLUSIONS, covered_tables, excluded_tables, tenant_scoped_tables, uncovered_tenant_tables};

/// The drift check itself: every tenant-scoped table is an archive source or
/// an exclusion, and the failure names what is missing.
fn assert_archive_accounts_for_every_table(conn: &rusqlite::Connection) {
    let uncovered = uncovered_tenant_tables(conn, &covered_tables(), &excluded_tables());
    assert!(
        uncovered.is_empty(),
        "tenant-scoped tables neither archived nor excluded (add an ArchiveEntry class or an EXPORT_EXCLUSIONS row): {}",
        uncovered.join(", ")
    );
}

#[test]
fn expc_ac02() {
    let dir = std::env::temp_dir().join(format!("expc-ac02-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let _db = Db::open(&dir).expect("open migrated db");
    let conn = rusqlite::Connection::open(dir.join("mcphost.db")).expect("raw conn");

    // As shipped: nothing is left over.
    assert_archive_accounts_for_every_table(&conn);

    // Exclusions are a clean list: no duplicates, none that is also an archive
    // source, none for a table the schema no longer has.
    let schema: BTreeSet<String> = tenant_scoped_tables(&conn).expect("walk").into_iter().collect();
    let excluded = excluded_tables();
    let distinct: BTreeSet<&str> = excluded.iter().copied().collect();
    assert_eq!(distinct.len(), EXPORT_EXCLUSIONS.len(), "EXPORT_EXCLUSIONS lists a table twice");
    let covered = covered_tables();
    for table in &excluded {
        assert!(!covered.contains(table), "{table} is both an archive source and an exclusion");
        assert!(schema.contains(*table), "{table} is excluded but is not a tenant-scoped table");
    }

    // A test copy with one more tenant-scoped table: the check fails, naming it.
    conn.execute_batch("CREATE TABLE foo (id INTEGER PRIMARY KEY, tenant_id INTEGER NOT NULL)").expect("add foo");
    let failure = catch_unwind(AssertUnwindSafe(|| assert_archive_accounts_for_every_table(&conn)))
        .expect_err("a table with tenant_id that is neither archived nor excluded must fail the check");
    let message = failure
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| failure.downcast_ref::<&str>().map(|s| s.to_string()))
        .expect("panic message");
    assert!(message.contains("foo"), "the failure names the table: {message}");
    // `*_tenant_id` alone also counts (`messages` scopes by `from_tenant_id`).
    conn.execute_batch("CREATE TABLE bar (id INTEGER PRIMARY KEY, owner_tenant_id INTEGER)").expect("add bar");
    let uncovered = uncovered_tenant_tables(&conn, &covered, &excluded);
    assert_eq!(uncovered, vec!["bar".to_string(), "foo".to_string()]);
    let _ = std::fs::remove_dir_all(&dir);
}
