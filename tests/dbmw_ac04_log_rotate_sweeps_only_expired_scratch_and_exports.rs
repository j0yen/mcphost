//! PRD-mcphost-db-maintenance-window AC4 (P0): `LogRotate` removes exactly the
//! expired `scratch/` and `exports/` entries (5 + 1), leaves the database,
//! `envs/`, `public/` and the signing key byte-identical, and reports
//! `journal_vacuum_requested: true`.

use crate::common;

use std::path::Path;
use std::time::{Duration, SystemTime};

use mcphost::heal::{HealAction, HealOutcome};
use mcphost::maintenance::{run_log_rotate, run_window};

fn age(path: &Path, secs: u64) {
    let f = std::fs::File::open(path).expect("open to age");
    f.set_modified(SystemTime::now() - Duration::from_secs(secs)).expect("set mtime");
}

fn hash_tree(path: &Path) -> u64 {
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

#[tokio::test]
async fn log_rotate_removes_only_expired_scratch_and_export_entries() {
    let (state, dir) = common::bare_app_state().await;
    let root = &dir.0;
    let day = 24 * 3600;

    std::fs::create_dir_all(root.join("scratch")).unwrap();
    for i in 0..5 {
        let p = root.join(format!("scratch/old-{i}"));
        if i % 2 == 0 {
            std::fs::write(&p, b"old").unwrap();
        } else {
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join("inner"), b"x").unwrap();
        }
        age(&p, day + 3600);
    }
    for i in 0..3 {
        let p = root.join(format!("scratch/new-{i}"));
        std::fs::write(&p, b"new").unwrap();
        age(&p, 3600);
    }
    std::fs::create_dir_all(root.join("exports")).unwrap();
    let expired = root.join("exports/expired.tar.gz");
    std::fs::write(&expired, b"zip").unwrap();
    age(&expired, (mcphost::export::EXPORT_URL_TTL_SECS + 3600) as u64);
    let live = root.join("exports/live.tar.gz");
    std::fs::write(&live, b"zip").unwrap();

    for d in ["envs", "public"] {
        std::fs::create_dir_all(root.join(d)).unwrap();
        std::fs::write(root.join(d).join("keep"), d.as_bytes()).unwrap();
        age(&root.join(d).join("keep"), 10 * day);
    }
    std::fs::write(root.join("authz_signing_key.der"), b"key-bytes").unwrap();
    age(&root.join("authz_signing_key.der"), 10 * day);

    let protected = ["envs", "public", "authz_signing_key.der"];
    let hashes: Vec<u64> = protected.iter().map(|p| hash_tree(&root.join(p))).collect();
    let db_before = std::fs::read(root.join("mcphost.db")).unwrap();
    let wal_before = std::fs::read(root.join("mcphost.db-wal")).unwrap_or_default();

    // The sweep alone, hash-compared: it writes no database byte.
    let now = mcphost::state::now_unix();
    let evidence = run_log_rotate(&state.db, now).await.expect("sweep");
    assert_eq!(evidence["after"]["scratch_removed"], 5);
    assert_eq!(evidence["after"]["exports_removed"], 1);
    assert_eq!(evidence["after"]["journal_vacuum_requested"], true);
    assert_eq!(evidence["before"]["scratch_entries"], 8);
    assert_eq!(evidence["after"]["scratch_entries"], 3);
    assert_eq!(db_before, std::fs::read(root.join("mcphost.db")).unwrap(), "mcphost.db changed");
    assert_eq!(wal_before, std::fs::read(root.join("mcphost.db-wal")).unwrap_or_default(), "mcphost.db-wal changed");

    // And through the window, the row carries the same evidence (a second
    // sweep finds nothing left, so the counts it reports are 0 removed).
    let report = run_window(&state.db, now, false).await.expect("window");
    let row = report.rows.iter().find(|r| r.action == HealAction::LogRotate).expect("log_rotate row");
    assert_eq!(row.outcome, HealOutcome::Ok, "{row:?}");
    assert_eq!(row.after["journal_vacuum_requested"], true);

    assert!(!expired.exists() && live.exists());
    let left: Vec<_> = std::fs::read_dir(root.join("scratch")).unwrap().flatten().map(|e| e.file_name()).collect();
    assert_eq!(left.len(), 3, "{left:?}");
    assert!(left.iter().all(|n| n.to_string_lossy().starts_with("new-")));

    let after: Vec<u64> = protected.iter().map(|p| hash_tree(&root.join(p))).collect();
    assert_eq!(hashes, after, "protected paths changed");
}
