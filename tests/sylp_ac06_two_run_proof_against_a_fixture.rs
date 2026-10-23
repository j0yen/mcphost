//! contract: reads examples/data-sync/orders.csv, examples/data-sync/proof.sh
//! sylp AC6 (PRD-mcphost-sync-pull-change-detection-live-proof R6): the
//! two-run `examples/data-sync/proof.sh` against the in-process server and a
//! local fixture CSV server (`DATA_SYNC_CSV_URL`): an unchanged second fetch
//! ends `RESULT: PASS` with the second get line reading
//! `status=ok rows_added=0 change_detected=false`; a second fetch answering
//! 500 ends `RESULT: FAIL` naming the second get's failed expect.
//! (`tests/dsproof_data_sync_proof.rs` holds the single-run proof tests.)

use crate::common::sypul::{Later, TwoFetch};
use crate::common::TestServer;
use std::path::Path;
use std::process::Output;
use tokio::process::Command;

fn orders_csv() -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/data-sync/orders.csv")).expect("read orders.csv")
}

fn transcript(o: &Output) -> String {
    format!("{}\n--- stderr ---\n{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

/// Runs the committed proof against `server` with the CSV at `csv_url`. Async
/// `Command`: a blocking `output()` would starve the in-process server.
async fn run_proof(server: &TestServer, csv_url: &str) -> Output {
    Command::new("bash")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/data-sync/proof.sh"))
        .env_remove("MCPHOST_PROVE_DRY")
        .env_remove("SYNTHETIC_HEADER")
        .env_remove("DATA_SYNC_ROWS")
        .env("MCPHOST_URL", format!("{}/mcp", server.base_url))
        .env("DATA_SYNC_CSV_URL", csv_url)
        .output()
        .await
        .expect("run proof.sh")
}

fn get_lines(stdout: &str) -> Vec<&str> {
    stdout.lines().filter(|l| l.starts_with("host.sync.get: ")).collect()
}

#[tokio::test]
async fn sylp_ac06_unchanged_second_fetch_passes_with_no_change_receipt() {
    let server = TestServer::start().await;
    let fixture = TwoFetch::start(&orders_csv(), Later::Unchanged).await;
    // Two ticks (30 s each) plus the polls, within the recipe's wall_s.
    let out = crate::deadline_budget!(150_000, { run_proof(&server, &fixture.csv_url()).await });
    let t = transcript(&out);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(out.status.success() && stdout.contains("RESULT: PASS"), "{t}");
    let gets = get_lines(&stdout);
    assert_eq!(gets.len(), 2, "two host.sync.get: lines\n{t}");
    assert!(gets[0].contains("last_run.status=ok rows_added=12 change_detected=true waited_s="), "{}\n{t}", gets[0]);
    assert!(gets[1].contains("last_run.status=ok rows_added=0 change_detected=false waited_s="), "{}\n{t}", gets[1]);
    let run_id = |l: &str| l.split("last_run.run_id=").nth(1).and_then(|r| r.split(' ').next()).unwrap_or("").to_string();
    assert!(!run_id(gets[0]).is_empty() && run_id(gets[0]) != run_id(gets[1]), "the two receipts are different runs\n{t}");
    assert_eq!(stdout.matches("host.sync.run_now:").count(), 2, "{t}");
}

#[tokio::test]
async fn sylp_ac06_second_fetch_500_fails_naming_the_second_get_expect() {
    let server = TestServer::start().await;
    let fixture = TwoFetch::start(&orders_csv(), Later::ServerError).await;
    // The second get polls its whole retry_s before it gives up.
    let out = crate::deadline_budget!(190_000, { run_proof(&server, &fixture.csv_url()).await });
    let t = transcript(&out);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(out.status.code(), Some(1), "{t}");
    assert!(stdout.contains("RESULT: FAIL"), "{t}");
    // The first run still reads as a clean load; only the second get fails.
    let gets = get_lines(&stdout);
    assert!(gets[0].contains("last_run.status=ok rows_added=12 change_detected=true"), "{t}");
    assert!(stdout.contains("CHECK get_last_run_status_is_ok: PASS"), "{t}");
    assert!(stdout.contains("CHECK get2_expects_hold: FAIL"), "{t}");
    assert!(stdout.contains("FAILED EXPECT (2nd host.sync.get): last_run.status is "), "{t}");
    assert!(stderr.contains("host.sync.get (2nd): still failing after") && stderr.contains("want ok"), "{t}");
    assert!(!stdout.contains("FAILED EXPECT (1st"), "{t}");
}
