//! PRD-mcphost-funnel-by-surface-nightly-probe-b
//! AC3 (P1) — Given `mcphost funnel --surface cursor`, When run, Then it
//! prints one table with the five stages for that surface and exits 0; an
//! unknown surface exits 2 naming the valid ids.

use crate::common::TempDataDir;
use mcphost::auth::{generate_key, generate_namespace, hash_key};
use mcphost::db::Db;

fn run(dir: &TempDataDir, args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .arg("funnel")
        .args(args)
        .env("MCPHOST_DATA_DIR", &dir.0)
        .output()
        .expect("spawn mcphost funnel")
}

#[tokio::test]
async fn surface_flag_prints_one_table_and_unknown_surface_exits_2() {
    let dir = TempDataDir::new();
    {
        let db = Db::open(&dir.0).expect("open db");
        db.migrate().await.expect("migrate");
        db.create_tenant_attributed_with_source(
            "A".to_string(),
            generate_namespace(),
            hash_key(&generate_key()),
            None,
            Some("external".to_string()),
            Some("cursor".to_string()),
            None,
            "external".to_string(),
            None,
            None,
            "human".to_string(),
        )
        .await
        .expect("create tenant");
    }

    let ok = run(&dir, &["--surface", "cursor"]);
    assert_eq!(ok.status.code(), Some(0), "stderr={}", String::from_utf8_lossy(&ok.stderr));
    let out = String::from_utf8_lossy(&ok.stdout);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "one header + one row: {out}");
    for stage in ["signed_up", "claimed", "published", "first_external_call", "day7_active"] {
        assert!(lines[0].contains(stage), "header lacks {stage}: {out}");
    }
    assert!(lines[1].starts_with("cursor"), "{out}");
    assert_eq!(lines[1].split_whitespace().nth(1), Some("1"), "signed_up of the planted tenant: {out}");

    let bad = run(&dir, &["--surface", "notasurface"]);
    assert_eq!(bad.status.code(), Some(2));
    let err = String::from_utf8_lossy(&bad.stderr);
    for id in mcphost::install_links::SURFACES {
        assert!(err.contains(id), "stderr must name valid id {id}: {err}");
    }
}
