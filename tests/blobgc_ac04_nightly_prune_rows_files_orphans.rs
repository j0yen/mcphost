//! PRD-mcphost-blob-retention-and-quota
//! AC4 -- Given 10,000 expired blobs and 2 orphan files, When the nightly
//! prune runs, Then all 10,000 rows and files are gone, the 2 orphans are
//! unlinked, `prune_log` has a `blobs` row with those counts, and the
//! SQL-table prune entries are unchanged.

use crate::common;
use std::path::Path;

const EXPIRED: i64 = 10_000;

fn old(path: &Path) {
    let f = std::fs::OpenOptions::new().write(true).open(path).expect("open");
    f.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(7200)).expect("set mtime");
}

#[tokio::test]
async fn prune_removes_expired_rows_and_files_and_unlinks_orphans() {
    let server = common::TestServer::start().await;
    let (namespace, _key) = common::signup(&server.base_url, "Blobgc AC4").await;
    let tenant = server.state.db.find_tenant_by_namespace(namespace).await.unwrap().unwrap();
    let data_dir = server.state.db.data_dir().to_path_buf();
    let dir = data_dir.join("blobs").join(tenant.id.to_string());
    std::fs::create_dir_all(&dir).unwrap();

    // A control row on the SQL side so "unchanged" has something to compare.
    let now = mcphost::state::now_unix();
    server.state.db.set_retention_days_for_test("calls".to_string(), 90).await.unwrap();
    // PRD-mcphost-call-audit-export R4: a Free tenant now keeps 7 days of
    // `calls`; this test is about the host-wide 90-day window, so its tenant
    // is on Pro (90 days) and only that window applies.
    server
        .state
        .db
        .upgrade_tenant_plan(tenant.id, "pro".to_string(), "2026-10-01T00:00:00Z".to_string(), None)
        .await
        .expect("move tenant to pro");
    server.state.db.insert_calls_row_for_test(tenant.id, now - 91 * 86_400).await.unwrap();
    server.state.db.insert_calls_row_for_test(tenant.id, now - 89 * 86_400).await.unwrap();

    // 10,000 blobs that lapsed three days ago (past the 1-day grace), plus
    // one live blob that must survive.
    let conn = rusqlite::Connection::open(data_dir.join("mcphost.db")).unwrap();
    conn.execute_batch("BEGIN").unwrap();
    {
        let mut ins = conn
            .prepare(
                "INSERT INTO blobs (id, tenant_id, name, version, mime, bytes, sha256, created_at, \
                     updated_at, expires_at) VALUES (?1, ?2, ?3, 1, 'text/plain', 4, 'x', ?4, ?4, ?5)",
            )
            .unwrap();
        for i in 0..EXPIRED {
            let id = format!("EXPIRED{i:06}");
            ins.execute(rusqlite::params![id, tenant.id, format!("old-{i}.txt"), now - 10 * 86_400, now - 3 * 86_400])
                .unwrap();
            std::fs::write(dir.join(&id), b"gone").unwrap();
        }
        ins.execute(rusqlite::params!["LIVE000000", tenant.id, "live.txt", now, now + 86_400]).unwrap();
        std::fs::write(dir.join("LIVE000000"), b"live").unwrap();
    }
    conn.execute_batch("COMMIT").unwrap();
    for orphan in ["ORPHAN0001", "ORPHAN0002"] {
        std::fs::write(dir.join(orphan), b"stray").unwrap();
        old(&dir.join(orphan));
    }
    // A fresh stray file may be a put in flight: left for the next night.
    std::fs::write(dir.join("INFLIGHT01"), b"new").unwrap();

    // Guardrail: prune of {EXPIRED} blobs within 30 s (3x under MCPHOST_PERF_SKIP=1).
    let report =
        crate::deadline_budget!(30_000, { server.state.db.prune_once().await.expect("prune_once") });

    assert_eq!(report.deleted.get("blobs").copied(), Some(EXPIRED), "{:?}", report.deleted);
    assert_eq!(report.deleted.get("blobs_orphans").copied(), Some(2), "{:?}", report.deleted);
    assert_eq!(report.deleted.get("blobs_bytes").copied(), Some(EXPIRED * 4), "{:?}", report.deleted);
    assert_eq!(report.deleted.get("calls").copied(), Some(1), "SQL-table entries unchanged: {:?}", report.deleted);

    let rows: i64 = conn.query_row("SELECT COUNT(*) FROM blobs", [], |r| r.get(0)).unwrap();
    assert_eq!(rows, 1, "only the live row remains");
    let mut files: Vec<String> =
        std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    files.sort();
    assert_eq!(files, vec!["INFLIGHT01".to_string(), "LIVE000000".to_string()]);

    // prune_log carries the same counts.
    let logged: String = conn
        .query_row("SELECT deleted_json FROM prune_log ORDER BY id DESC LIMIT 1", [], |r| r.get(0))
        .unwrap();
    let logged: serde_json::Value = serde_json::from_str(&logged).unwrap();
    assert_eq!(logged["blobs"], EXPIRED);
    assert_eq!(logged["blobs_orphans"], 2);
}

#[tokio::test]
async fn a_row_whose_file_is_gone_is_marked_missing() {
    let server = common::TestServer::start().await;
    let (namespace, _key) = common::signup(&server.base_url, "Blobgc AC4 missing").await;
    let tenant = server.state.db.find_tenant_by_namespace(namespace).await.unwrap().unwrap();
    let conn = rusqlite::Connection::open(server.state.db.data_dir().join("mcphost.db")).unwrap();
    let now = mcphost::state::now_unix();
    conn.execute(
        "INSERT INTO blobs (id, tenant_id, name, version, mime, bytes, sha256, created_at, updated_at) \
         VALUES ('NOFILE0001', ?1, 'nofile.txt', 1, 'text/plain', 4, 'x', ?2, ?2)",
        rusqlite::params![tenant.id, now],
    )
    .unwrap();
    let report = server.state.db.prune_once().await.expect("prune_once");
    assert_eq!(report.deleted.get("blobs_missing").copied(), Some(1), "{:?}", report.deleted);
    let missing: i64 =
        conn.query_row("SELECT missing FROM blobs WHERE id = 'NOFILE0001'", [], |r| r.get(0)).unwrap();
    assert_eq!(missing, 1);
}
