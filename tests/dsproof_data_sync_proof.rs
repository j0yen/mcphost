//! Hand PR `data-sync-proof-recipe` (evidence for
//! PRD-mcphost-sync-source-registry-and-create live AC 8): the generated
//! `examples/data-sync/proof.sh`.
//!
//! (a) the dry run prints its plan and exits 0; (b) a full run against a local
//! `TestServer` exits 0, creates then deletes the sync, and its tenant carries
//! the `recipe:data-sync` synthetic stamp; (c) the committed proof is exactly
//! what `cargo xtask recipes` generates; (d) an empty `SYNTHETIC_HEADER`
//! against a non-loopback `MCPHOST_URL` is refused before any network call.
//! No test touches a real host: (a) and (d) exit before the first curl, (b)
//! talks to the in-process server only.

use crate::common;

use common::TestServer;
use rusqlite::params;
use std::path::{Path, PathBuf};
use std::process::Output;
use tokio::process::Command;

fn repo() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn script() -> PathBuf {
    repo().join("examples/data-sync/proof.sh")
}

fn proof() -> Command {
    let mut cmd = Command::new("bash");
    cmd.arg(script()).env_remove("MCPHOST_PROVE_DRY").env_remove("SYNTHETIC_HEADER").env_remove("DATA_SYNC_CSV_URL");
    cmd
}

fn transcript(o: &Output) -> String {
    format!("{}\n--- stderr ---\n{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

#[tokio::test]
async fn dsproof_ac01_dry_run_prints_the_plan_and_exits_zero() {
    let out = proof()
        .env("MCPHOST_PROVE_DRY", "1")
        .env("MCPHOST_URL", "https://dsproof.invalid/mcp")
        .output()
        .await
        .expect("run proof.sh");
    let t = transcript(&out);
    assert!(out.status.success(), "dry run exited non-zero:\n{t}");
    assert!(String::from_utf8_lossy(&out.stdout).contains("plan: data-sync proof against https://dsproof.invalid/mcp"), "{t}");
}

#[tokio::test]
async fn dsproof_ac02_full_run_creates_then_deletes_the_sync_on_a_synthetic_tenant() {
    let server = TestServer::start().await;
    // Async `Command`: a blocking `output()` would starve the in-process
    // server's own axum task.
    // The get step polls for the tick's receipt (up to 75 s, tick every 30 s).
    let out = crate::deadline_budget!(100_000, {
        proof().env("MCPHOST_URL", format!("{}/mcp", server.base_url)).output().await.expect("run proof.sh")
    });
    let t = transcript(&out);
    assert!(out.status.success(), "proof.sh exited non-zero:\n{t}");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(stdout.contains("RESULT: PASS"), "{t}");
    let create = stdout.lines().find(|l| l.starts_with("host.sync.create: ")).unwrap_or_else(|| panic!("no create line:\n{t}"));
    assert!(create.contains("sync_id=") && create.contains(" table=orders ") && create.contains("existing=false"), "{create}");
    assert!(stdout.contains("host.sync.run_now: next_run_unix="), "{t}");
    // The tick wrote a receipt for this sync: both ids are non-empty.
    let get = stdout.lines().find(|l| l.starts_with("host.sync.get: ")).unwrap_or_else(|| panic!("no get line:\n{t}"));
    assert!(get.contains("table=orders ") && !get.contains("last_run.run_id= ") && !get.ends_with("last_run.started_unix="), "{get}");
    assert!(stdout.contains("host.sync.delete: deleted=true"), "{t}");

    // The proof's tenant carries the explicit synthetic stamp (a loopback
    // signup is synthetic anyway; only the header gives it this label).
    let ns = stdout
        .lines()
        .find_map(|l| l.strip_prefix("OWNER_NS="))
        .unwrap_or_else(|| panic!("no OWNER_NS line:\n{t}"))
        .to_string();
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("query").expect("tenant exists");
    assert_eq!(tenant.origin, "synthetic");
    assert_eq!(tenant.origin_detail.as_deref(), Some("recipe:data-sync"));

    // The delete step left no schedule behind; the mirrored table stays.
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("open raw db");
    let syncs: i64 = conn
        .query_row("SELECT COUNT(*) FROM syncs WHERE tenant_id = ?1", params![tenant.id], |r| r.get(0))
        .expect("count syncs");
    assert_eq!(syncs, 0, "host.sync.delete must remove the sync");
}

#[test]
fn dsproof_ac03_committed_proof_is_what_the_generator_emits() {
    let files = mcphost::recipes::plan(repo()).expect("recipes plan");
    let (_, want) = files
        .iter()
        .find(|(p, _)| p == Path::new("examples/data-sync/proof.sh"))
        .expect("the generator plans examples/data-sync/proof.sh");
    let got = std::fs::read_to_string(script()).expect("read proof.sh");
    assert_eq!(&got, want, "examples/data-sync/proof.sh drifted; run `cargo xtask recipes`");
    assert!(got.starts_with("#!/usr/bin/env bash\n# GENERATED from recipes/data-sync.toml by `cargo xtask recipes` -- do not hand-edit.\n"));
    let drift = mcphost::recipes::drift(repo()).expect("recipes drift");
    assert!(drift.is_empty(), "recipes --check would fail: {drift:?}");
}

#[tokio::test]
async fn dsproof_ac04_empty_synthetic_header_against_a_real_host_is_refused() {
    // `.invalid` never resolves: if the guard ever failed to fire, the signup
    // would fail to connect rather than reach a real host.
    let out = proof()
        .env("SYNTHETIC_HEADER", "")
        .env("MCPHOST_URL", "https://dsproof.invalid/mcp")
        .output()
        .await
        .expect("run proof.sh");
    let t = transcript(&out);
    assert_eq!(out.status.code(), Some(2), "{t}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("SYNTHETIC_HEADER empty: refusing a bare signup against https://dsproof.invalid/mcp"),
        "{t}"
    );
    assert!(!String::from_utf8_lossy(&out.stdout).contains("CHECK "), "no check may run before the refusal:\n{t}");
}

#[tokio::test]
async fn dsproof_ac05_userinfo_before_at_does_not_pass_the_loopback_guard() {
    // `127.0.0.1:80@host` parses as host `host`; the guard must refuse it like
    // any non-loopback URL when the synthetic header is empty.
    let out = proof()
        .env("SYNTHETIC_HEADER", "")
        .env("MCPHOST_URL", "http://127.0.0.1:80@dsproof.invalid/mcp")
        .output()
        .await
        .expect("run proof.sh");
    let t = transcript(&out);
    assert_eq!(out.status.code(), Some(2), "{t}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("SYNTHETIC_HEADER empty: refusing a bare signup"), "{t}");
    assert!(!String::from_utf8_lossy(&out.stdout).contains("CHECK "), "no check may run before the refusal:\n{t}");
}

#[tokio::test]
async fn dsproof_ac06_a_terminated_run_still_deletes_its_sync() {
    use std::process::Stdio;
    use tokio::io::{AsyncBufReadExt, BufReader};

    let server = TestServer::start().await;
    let mut child = proof()
        .env("MCPHOST_URL", format!("{}/mcp", server.base_url))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn proof.sh");
    let pid = child.id().expect("child pid");
    let mut lines = BufReader::new(child.stdout.take().expect("piped stdout")).lines();
    let mut ns = None;
    let mut created = false;
    let mut seen = String::new();
    crate::deadline_budget!(30_000, {
        while let Some(l) = lines.next_line().await.expect("read proof stdout") {
            seen.push_str(&l);
            seen.push('\n');
            if let Some(n) = l.strip_prefix("OWNER_NS=") {
                ns = Some(n.to_string());
            }
            if l.starts_with("host.sync.run_now: ") {
                created = true;
                break;
            }
        }
    });
    assert!(created, "the proof never reached the run_now step:\n{seen}");
    // SIGTERM once the sync exists and the proof is polling for its receipt: the EXIT trap must delete it.
    let st = Command::new("kill").arg("-TERM").arg(pid.to_string()).status().await.expect("kill");
    assert!(st.success() || child.try_wait().ok().flatten().is_some());
    crate::deadline_budget!(30_000, {
        while lines.next_line().await.expect("drain proof stdout").is_some() {}
        child.wait().await.expect("wait proof.sh");
    });
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns.expect("OWNER_NS line"))
        .await
        .expect("query")
        .expect("tenant exists");
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("open raw db");
    let syncs: i64 = conn
        .query_row("SELECT COUNT(*) FROM syncs WHERE tenant_id = ?1", params![tenant.id], |r| r.get(0))
        .expect("count syncs");
    assert_eq!(syncs, 0, "a terminated proof must not leave its sync behind");
}
