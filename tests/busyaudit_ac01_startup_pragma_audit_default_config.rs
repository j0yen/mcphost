//! PRD-mcphost-sqlite-busy-timeout-audit
//! AC1 — Given the server starts with default config, When it opens its
//! connections, Then the log shows one audit line per role with
//! `busy_timeout=5000 journal_mode=wal synchronous=normal foreign_keys=1`
//! read back from the connection.

use crate::busyaudit;

use busyaudit::{capture_tracing, scratch_data_dir};
use mcphost::db::{Db, DbConfig};

#[test]
fn startup_audit_logs_one_line_per_role_with_default_pragmas() {
    let data_dir = scratch_data_dir("busyaudit-ac01");

    let (db, log) = capture_tracing(|| Db::open_with_cfg(&data_dir, DbConfig::default()));
    let _db = db.expect("Db::open with default config");

    let expected = "busy_timeout=5000 journal_mode=wal synchronous=normal foreign_keys=1";
    assert!(
        log.contains(&format!("role=server {expected}")),
        "expected a role=server audit line in:\n{log}"
    );
    assert!(
        log.contains(&format!("role=prune {expected}")),
        "expected a role=prune audit line in:\n{log}"
    );

    let _ = std::fs::remove_dir_all(&data_dir);
}
