//! PRD-mcphost-export-data-class-completeness-v2
//! AC1 -- Given a migrated db, When the schema walk runs with its covered
//! set fixed to the source tables of the 14 `ArchiveEntry` variants shipped
//! at 6c0de78 and an empty exclusions list, Then the uncovered list is
//! non-empty and includes `calls`, `tenant_audit` and `blobs` (falsifier: the
//! archive is not complete today).

use mcphost::db::Db;
use mcphost::export::uncovered_tenant_tables;

/// The main-db tables the 14 variants shipped at 6c0de78 read, fixed here as a
/// literal so this test keeps describing the archive as it was then.
const SHIPPED_AT_6C0DE78: &[&str] = &[
    "tools",              // ToolConfig, ToolSource
    "tenant_state_kv",    // State
    "documents",          // Document, Doc, DocMeta
    "document_blobs",     // Document, Doc, DocMeta
    "secrets",            // Secrets (names)
    "runs",               // Runs
    "messages",           // Threads
    "message_receipts",   // Threads
    "table_model_annotations", // Model
];

#[test]
fn expc_ac01() {
    let dir = std::env::temp_dir().join(format!("expc-ac01-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let _db = Db::open(&dir).expect("open migrated db");
    let conn = rusqlite::Connection::open(dir.join("mcphost.db")).expect("raw conn");

    let uncovered = uncovered_tenant_tables(&conn, SHIPPED_AT_6C0DE78, &[]);
    let _ = std::fs::remove_dir_all(&dir);

    assert!(!uncovered.is_empty(), "the archive is complete today: nothing to build");
    for table in ["calls", "tenant_audit", "blobs"] {
        assert!(uncovered.iter().any(|t| t == table), "{table} must be uncovered at 6c0de78: {uncovered:?}");
    }
    // `messages` scopes by `from_tenant_id` only; the walk matches that column,
    // and `ArchiveEntry::Threads` covers it.
    assert!(!uncovered.iter().any(|t| t == "messages"), "messages are covered by Threads: {uncovered:?}");
}
