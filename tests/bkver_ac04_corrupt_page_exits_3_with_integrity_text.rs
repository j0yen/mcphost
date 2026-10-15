//! PRD-mcphost-backup-restore-proof-nightly
//! AC4 — Given a file with a corrupted page, When digested, Then exit 3
//! with `integrity` holding the `PRAGMA integrity_check` text and no panic.

use crate::common;
use common::TempDataDir;
use mcphost::db::Db;
use std::io::{Seek, SeekFrom, Write};
use std::process::Command;

#[tokio::test]
async fn bkver_ac04() {
    let dir = TempDataDir::new();
    let db = Db::open(&dir.0).expect("open db");
    db.migrate().await.expect("migrate");
    let snap = dir.0.join("snapshot.db");
    {
        let conn = rusqlite::Connection::open(dir.0.join("mcphost.db")).expect("open");
        conn.execute("VACUUM INTO ?1", [snap.to_str().unwrap()]).expect("snapshot");
    }

    // Overwrite the middle of a later page (a b-tree interior of the schema
    // or table data) with garbage; the header and page 1 stay readable.
    let len = std::fs::metadata(&snap).expect("meta").len();
    assert!(len > 4096 * 8, "snapshot has many pages");
    let mut f = std::fs::OpenOptions::new().write(true).open(&snap).expect("open snapshot");
    f.seek(SeekFrom::Start(4096 * 3)).expect("seek");
    f.write_all(&[0xFF; 4096]).expect("corrupt page");
    drop(f);

    let o = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["backup", "digest", "--json", "--db"])
        .arg(&snap)
        .output()
        .expect("run digest");
    assert_eq!(o.status.code(), Some(3), "exit 3, not a panic: {}", String::from_utf8_lossy(&o.stderr));
    assert!(!String::from_utf8_lossy(&o.stderr).contains("panicked"));
    let report: serde_json::Value = serde_json::from_slice(&o.stdout).expect("report json");
    let integrity = report["integrity"].as_str().expect("integrity text");
    assert_ne!(integrity, "ok");

    // The text is exactly what PRAGMA integrity_check says for that file.
    let conn = rusqlite::Connection::open_with_flags(&snap, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("open ro");
    let expected = conn
        .prepare("PRAGMA integrity_check")
        .and_then(|mut s| {
            s.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()
        })
        .map(|v| v.join("\n"));
    if let Ok(expected) = expected {
        assert_eq!(integrity, expected);
    }
    assert!(report["digest"].is_null());

    // Not a database at all: still exit 3 with text, no panic.
    let junk = dir.0.join("junk.db");
    std::fs::write(&junk, vec![0x41u8; 20_000]).expect("junk");
    let o = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["backup", "digest", "--json", "--db"])
        .arg(&junk)
        .output()
        .expect("run digest junk");
    assert_eq!(o.status.code(), Some(3));
    let report: serde_json::Value = serde_json::from_slice(&o.stdout).expect("junk report");
    assert!(!report["integrity"].as_str().unwrap_or("").is_empty());
}
