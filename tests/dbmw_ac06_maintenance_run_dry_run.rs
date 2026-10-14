//! PRD-mcphost-db-maintenance-window AC6 (P0): `mcphost maintenance run
//! --dry-run` prints `before` values for both actions, writes two `would_run`
//! rows and changes no file or pragma (hash-compared).

use crate::common;

use std::path::Path;
use std::process::Command;

use rusqlite::Connection;

fn hash_dir(path: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    fn walk(p: &Path, h: &mut impl Hasher) {
        if p.is_dir() {
            let mut kids: Vec<_> = std::fs::read_dir(p).unwrap().flatten().map(|e| e.path()).collect();
            kids.sort();
            for k in kids {
                k.file_name().hash(h);
                walk(&k, h);
            }
        } else {
            std::fs::read(p).unwrap().hash(h);
        }
    }
    walk(path, &mut h);
    h.finish()
}

fn pragmas(db: &Path) -> Vec<(String, String)> {
    let conn = Connection::open(db).unwrap();
    ["auto_vacuum", "journal_mode", "user_version", "page_size", "synchronous", "wal_autocheckpoint"]
        .iter()
        .map(|p| (p.to_string(), conn.query_row(&format!("PRAGMA {p}"), [], |r| r.get::<_, rusqlite::types::Value>(0)).map(|v| format!("{v:?}")).unwrap()))
        .collect()
}

#[test]
fn dry_run_prints_before_writes_two_would_run_rows_and_changes_nothing() {
    let dir = common::TempDataDir::new();
    {
        let db = mcphost::db::Db::open(&dir.0).expect("open");
        drop(db);
    }
    let root = &dir.0;
    std::fs::create_dir_all(root.join("scratch")).unwrap();
    std::fs::write(root.join("scratch/old"), b"x").unwrap();
    std::fs::File::open(root.join("scratch/old"))
        .unwrap()
        .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(3 * 86_400))
        .unwrap();
    std::fs::create_dir_all(root.join("exports")).unwrap();
    std::fs::write(root.join("exports/e.tar.gz"), b"z").unwrap();
    std::fs::write(root.join("authz_signing_key.der"), b"key").unwrap();

    let tracked = ["scratch", "exports", "authz_signing_key.der"];
    let hashes: Vec<u64> = tracked.iter().map(|p| hash_dir(&root.join(p))).collect();
    let pragmas_before = pragmas(&root.join("mcphost.db"));

    let out = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["maintenance", "run", "--dry-run"])
        .env("MCPHOST_DATA_DIR", root)
        .env_remove("MCPHOST_HEAL_DRY_RUN")
        .output()
        .expect("run mcphost maintenance");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "stdout: {stdout}\nstderr: {}", String::from_utf8_lossy(&out.stderr));

    let line = |action: &str| {
        stdout
            .lines()
            .find(|l| l.starts_with(&format!("{action} outcome=would_run")))
            .unwrap_or_else(|| panic!("no {action} line in:\n{stdout}"))
            .to_string()
    };
    let db_line = line("db_maintenance");
    for key in ["db_mb", "wal_mb", "page_count", "freelist_count"] {
        assert!(db_line.contains(&format!("\"{key}\"")), "{db_line}");
    }
    let log_line = line("log_rotate");
    assert!(log_line.contains("\"scratch_entries\":1"), "{log_line}");
    assert!(log_line.contains("\"exports_entries\":1"), "{log_line}");

    let conn = Connection::open(root.join("mcphost.db")).unwrap();
    let rows: Vec<(String, String, i64)> = conn
        .prepare("SELECT action, outcome, dry_run FROM incidents WHERE action IN ('db_maintenance', 'log_rotate') ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        rows,
        vec![("db_maintenance".into(), "would_run".into(), 1), ("log_rotate".into(), "would_run".into(), 1)]
    );

    let after: Vec<u64> = tracked.iter().map(|p| hash_dir(&root.join(p))).collect();
    assert_eq!(hashes, after, "files changed");
    assert_eq!(pragmas_before, pragmas(&root.join("mcphost.db")), "pragmas changed");
}
