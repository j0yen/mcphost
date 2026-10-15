//! PRD-mcphost-backup-restore-proof-nightly
//! AC3 — Given a copy with one `tools` row deleted, When both files are
//! digested and compared, Then `--compare` exits 4 naming `tools` with both
//! counts, and `admin.backup_verified` records `digest_match: false`.

use crate::common;
use common::{ADMIN_KEY, McpClient, TempDataDir, TestServer, extract_structured};
use mcphost::auth::{generate_key, generate_namespace, hash_key};
use mcphost::db::Db;
use serde_json::{json, Value};
use std::path::Path;
use std::process::Command;

fn digest_json(db: &Path, out: &Path) -> Value {
    let o = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["backup", "digest", "--json", "--db"])
        .arg(db)
        .output()
        .expect("run digest");
    assert_eq!(o.status.code(), Some(0), "digest ok: {}", String::from_utf8_lossy(&o.stderr));
    std::fs::write(out, &o.stdout).expect("write report");
    serde_json::from_slice(&o.stdout).expect("report json")
}

#[tokio::test]
async fn bkver_ac03() {
    let live_dir = TempDataDir::new();
    let db = Db::open(&live_dir.0).expect("open db");
    db.migrate().await.expect("migrate");
    let key = generate_key();
    let tenant = db
        .create_tenant_attributed(
            "Backup Tenant".to_string(),
            generate_namespace(),
            hash_key(&key),
            None,
            Some("external".to_string()),
            None,
            None,
            "external".to_string(),
            None,
        )
        .await
        .expect("tenant");
    for name in ["alpha", "beta", "gamma"] {
        db.upsert_tool(tenant.id, name.to_string(), "echo".to_string(), json!({}), 5, vec![])
            .await
            .expect("tool");
    }

    // The "restored copy": a consistent snapshot of the live file with one
    // `tools` row deleted afterwards.
    let live_path = live_dir.0.join("mcphost.db");
    let copy_dir = TempDataDir::new();
    let copy_path = copy_dir.0.join("copy.db");
    {
        let conn = rusqlite::Connection::open(&live_path).expect("open live");
        conn.execute("VACUUM INTO ?1", [copy_path.to_str().unwrap()]).expect("snapshot");
    }
    let (live_json, copy_json) = (copy_dir.0.join("live.json"), copy_dir.0.join("copy.json"));

    // An untouched copy matches: exit 0.
    let live_report = digest_json(&live_path, &live_json);
    let same_report = digest_json(&copy_path, &copy_dir.0.join("same.json"));
    assert_eq!(live_report["digest"], same_report["digest"]);
    let ok = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["backup", "digest", "--compare"])
        .arg(&live_json)
        .arg(copy_dir.0.join("same.json"))
        .output()
        .expect("compare equal");
    assert_eq!(ok.status.code(), Some(0));

    {
        let conn = rusqlite::Connection::open(&copy_path).expect("open copy");
        let n = conn.execute("DELETE FROM tools WHERE name = 'beta'", []).expect("delete");
        assert_eq!(n, 1);
    }
    let copy_report = digest_json(&copy_path, &copy_json);
    assert_eq!(live_report["counts"]["tools"], 3);
    assert_eq!(copy_report["counts"]["tools"], 2);
    assert_ne!(live_report["digest"], copy_report["digest"]);

    let cmp = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["backup", "digest", "--compare"])
        .arg(&live_json)
        .arg(&copy_json)
        .output()
        .expect("compare");
    assert_eq!(cmp.status.code(), Some(4), "mismatch exits 4");
    let out = String::from_utf8_lossy(&cmp.stdout);
    assert!(out.contains("mismatch: tools rows a=3 b=2"), "names tools with both counts: {out}");

    // admin.backup_verified records digest_match false for that pair.
    let server = TestServer::start().await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let taken = mcphost::state::now_unix() - 3600;
    let rec = extract_structured(
        &admin
            .tools_call(
                "admin.backup_verified",
                json!({
                    "source": "nightly",
                    "taken_at_unix": taken,
                    "live_digest": live_report["digest"],
                    "restored_digest": copy_report["digest"],
                    "restored_counts": copy_report["counts"],
                }),
            )
            .await
            .expect("backup_verified"),
    );
    assert_eq!(rec["digest_match"], false);
    assert!(rec["age_s"].as_i64().unwrap() >= 3600);
    let stored = {
        let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("open server db");
        conn.query_row(
            "SELECT digest_match, source FROM backup_verifications WHERE id = ?1",
            [rec["id"].as_i64().unwrap()],
            |r| Ok((r.get::<_, bool>(0)?, r.get::<_, String>(1)?)),
        )
        .expect("row")
    };
    assert_eq!(stored, (false, "nightly".to_string()));

    // Equal digests record true; a future or >7 d taken_at_unix is refused.
    let same = extract_structured(
        &admin
            .tools_call(
                "admin.backup_verified",
                json!({"source": "nightly", "taken_at_unix": taken,
                       "live_digest": "d", "restored_digest": "d"}),
            )
            .await
            .expect("match"),
    );
    assert_eq!(same["digest_match"], true);
    for bad in [mcphost::state::now_unix() + 3600, mcphost::state::now_unix() - 8 * 86_400] {
        let err = admin
            .tools_call(
                "admin.backup_verified",
                json!({"source": "nightly", "taken_at_unix": bad,
                       "live_digest": "d", "restored_digest": "d"}),
            )
            .await;
        let text = format!("{err:?}");
        assert!(text.contains("args_invalid"), "taken_at_unix {bad} refused: {text}");
    }
}
