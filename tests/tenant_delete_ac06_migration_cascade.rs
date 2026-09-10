//! PRD-mcphost-tenant-delete
//! AC6 — Given a database created at v0.5.5, When `mcphost migrate` runs,
//! Then the cascade migration applies, `PRAGMA foreign_key_list` on each
//! child table shows `ON DELETE CASCADE`, and existing rows are unchanged.

mod common;
use common::TempDataDir;
use rusqlite::params;

/// The pre-0005 schema (migrations 0001 + the 0002/0003/0004 `ALTER
/// TABLE`s, none of them cascading) exactly as v0.5.5 would have left it,
/// built by hand here (rather than via `Db::open`, which now applies 0005
/// immediately) so this test can prove the migration actually transforms
/// an old, non-cascading file rather than just checking a freshly created
/// one.
const OLD_SCHEMA: &str = "
CREATE TABLE tenants (
    id           INTEGER PRIMARY KEY,
    namespace    TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    key_hash     TEXT NOT NULL UNIQUE,
    created_at   TEXT NOT NULL,
    disabled     INTEGER NOT NULL DEFAULT 0,
    last_tool_change_unix INTEGER NOT NULL DEFAULT 0,
    namespace_verified INTEGER NOT NULL DEFAULT 0,
    registry_namespace TEXT
);
CREATE TABLE tools (
    id          INTEGER PRIMARY KEY,
    tenant_id   INTEGER NOT NULL REFERENCES tenants(id),
    name        TEXT NOT NULL,
    kind        TEXT NOT NULL,
    spec        TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    UNIQUE(tenant_id, name)
);
CREATE TABLE secrets (
    id          INTEGER PRIMARY KEY,
    tenant_id   INTEGER NOT NULL REFERENCES tenants(id),
    name        TEXT NOT NULL,
    value_enc   BLOB NOT NULL,
    nonce       BLOB NOT NULL,
    created_at  TEXT NOT NULL,
    UNIQUE(tenant_id, name)
);
CREATE TABLE calls (
    id           INTEGER PRIMARY KEY,
    tenant_id    INTEGER NOT NULL REFERENCES tenants(id),
    tool_name    TEXT NOT NULL,
    started_at   TEXT NOT NULL,
    started_unix INTEGER NOT NULL,
    duration_ms  INTEGER NOT NULL,
    ok           INTEGER NOT NULL,
    error_class  TEXT,
    cpu_ms       INTEGER,
    peak_rss_kb  INTEGER
);
CREATE TABLE logs (
    id         INTEGER PRIMARY KEY,
    tenant_id  INTEGER NOT NULL REFERENCES tenants(id),
    tool_name  TEXT NOT NULL,
    ts         TEXT NOT NULL,
    line       TEXT NOT NULL
);
CREATE TABLE signup_events (
    id         INTEGER PRIMARY KEY,
    source_ip  TEXT NOT NULL,
    created_unix INTEGER NOT NULL
);
CREATE TABLE registry_documents (
    tenant_id    INTEGER PRIMARY KEY REFERENCES tenants(id),
    namespace    TEXT NOT NULL,
    document     TEXT NOT NULL,
    published_at TEXT NOT NULL
);
";

#[tokio::test]
async fn migrate_adds_cascade_and_preserves_existing_rows() {
    let data_dir = TempDataDir::new();
    let db_path = data_dir.0.join("mcphost.db");

    // Build the old, non-cascading schema and seed one tenant + one tool,
    // exactly as a real v0.5.5 deployment would have on disk.
    {
        let conn = rusqlite::Connection::open(&db_path).expect("open raw db");
        conn.execute_batch(OLD_SCHEMA).expect("create old schema");
        conn.execute(
            "INSERT INTO tenants (id, namespace, display_name, key_hash, created_at, disabled) \
             VALUES (1, 'preexisting-ns', 'Pre-existing Tenant', 'hash-abc', 'unix:1.0', 0)",
            [],
        )
        .expect("seed tenant");
        conn.execute(
            "INSERT INTO tools (id, tenant_id, name, kind, spec, created_at) \
             VALUES (1, 1, 'legacy_tool', 'echo', '{}', 'unix:1.0')",
            [],
        )
        .expect("seed tool");
    }

    // `Db::open` runs `migrate()` (same path both `serve` and `mcphost
    // migrate` take) against that existing file.
    let db = mcphost::db::Db::open(&data_dir.0).expect("open db");
    db.migrate().await.expect("migrate");

    // Existing rows are unchanged.
    let tenant = db
        .find_tenant_by_namespace("preexisting-ns".to_string())
        .await
        .unwrap()
        .expect("pre-existing tenant survives the migration");
    assert_eq!(tenant.display_name, "Pre-existing Tenant");
    assert_eq!(tenant.key_hash, "hash-abc");
    let tools = db.list_tools(tenant.id).await.unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "legacy_tool");

    // `PRAGMA foreign_key_list` on each child table shows `ON DELETE
    // CASCADE` now -- a second, independent read connection to the same
    // (WAL-mode) file.
    //
    // Filtered on `"from" = 'tenant_id'`, not just `"table" = 'tenants'`:
    // `calls` has grown a second FK to `tenants` since this test was
    // written (`caller_tenant_id`, migration 0013, PRD-mcphost-sharing)
    // that is deliberately NOT this AC's concern -- it records who placed
    // a cross-tenant call, not who owns the row, and
    // `pragma_foreign_key_list` does not guarantee row order, so an
    // unfiltered query could silently read whichever FK SQLite lists
    // first (observed: it read `caller_tenant_id`'s plain `NO ACTION`
    // instead of `tenant_id`'s `CASCADE`). `tenant_id` is the one column
    // every one of these tables has always had and the one this AC's
    // cascade guarantee is about.
    let check = rusqlite::Connection::open(&db_path).expect("open raw db for pragma check");
    for table in ["logs", "calls", "secrets", "tools", "registry_documents"] {
        let on_delete: String = check
            .query_row(
                &format!(
                    "SELECT on_delete FROM pragma_foreign_key_list('{table}') \
                     WHERE \"table\" = 'tenants' AND \"from\" = 'tenant_id'"
                ),
                params![],
                |r| r.get(0),
            )
            .unwrap_or_else(|e| panic!("{table} has no FK to tenants after migration: {e}"));
        assert_eq!(on_delete, "CASCADE", "{table}'s FK must cascade");
    }
}
