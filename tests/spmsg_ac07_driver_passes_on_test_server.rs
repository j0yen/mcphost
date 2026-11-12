//! contract: reads scripts/live/spmsg_ac08.py
//! PRD-mcphost-invalid-spec-message-carries-fix
//! AC7 (P0) — Given an in-process `TestServer` with the http kind and a mint hook
//! handing the test the key it issues, When `scripts/live/spmsg_ac08.py` runs
//! under `python3 -I` against it with a results file in the test's tempdir, Then
//! it exits 0, its last stdout line is `refused=1 field_named=1
//! spec_in_message=1 republished=1 removed=1 offboarded=1`, no stdout or results
//! line holds that key, and no active tenant remains.
//!
//! The mint hook is a loopback forwarder between the driver and the server that
//! copies the server's bytes: the key the signup response issues is read from
//! that copy, so the server needs no test-only code path.

use crate::common::{TestServer, http_kind_registry};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};

const SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/live/spmsg_ac08.py");
const PASS: &str = "refused=1 field_named=1 spec_in_message=1 republished=1 removed=1 offboarded=1";

type Tee = Arc<Mutex<Vec<u8>>>;

/// Copies `from` to `to`, recording what passes when `tee` is set.
async fn pump(mut from: OwnedReadHalf, mut to: OwnedWriteHalf, tee: Option<Tee>) {
    let mut buf = [0u8; 8192];
    loop {
        match from.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if let Some(tee) = &tee {
                    tee.lock().unwrap().extend_from_slice(&buf[..n]);
                }
                if to.write_all(&buf[..n]).await.is_err() {
                    break;
                }
            }
        }
    }
    let _ = to.shutdown().await;
}

/// A forwarder to `upstream` on a fresh loopback port; returns its address and
/// the copy of every byte the upstream sent back.
async fn mint_hook(upstream: String) -> (String, Tee) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind forwarder");
    let addr = listener.local_addr().expect("addr").to_string();
    let seen: Tee = Arc::default();
    let tee = seen.clone();
    tokio::spawn(async move {
        while let Ok((client, _)) = listener.accept().await {
            let upstream = upstream.clone();
            let tee = tee.clone();
            tokio::spawn(async move {
                let Ok(server) = TcpStream::connect(&upstream).await else { return };
                let (cr, cw) = client.into_split();
                let (sr, sw) = server.into_split();
                tokio::join!(pump(cr, sw, None), pump(sr, cw, Some(tee)));
            });
        }
    });
    (addr, seen)
}

/// The key a signup response issued: the 64-hex value after a `"key":"` (or its
/// JSON-escaped form) in the recorded server bytes.
fn issued_key(seen: &Tee) -> String {
    let text = String::from_utf8_lossy(&seen.lock().unwrap()).to_string();
    for marker in ["\"key\":\"", "\\\"key\\\":\\\""] {
        if let Some(at) = text.find(marker) {
            let key: String = text[at + marker.len()..].chars().take_while(char::is_ascii_hexdigit).collect();
            if key.len() == 64 {
                return key;
            }
        }
    }
    panic!("no issued key in the recorded signup response: {text}");
}

fn active_tenants(server: &TestServer) -> i64 {
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    conn.query_row("SELECT count(*) FROM tenants WHERE disabled = 0", [], |r| r.get(0)).expect("count tenants")
}

fn run_driver(base_url: String, results: String) -> Output {
    Command::new("python3")
        .args(["-I", SCRIPT, "--base-url", &base_url, "--results", &results])
        .env("NO_PROXY", "127.0.0.1")
        .env("no_proxy", "127.0.0.1")
        .env("HTTPS_PROXY", "http://127.0.0.1:9")
        .output()
        .expect("run spmsg_ac08.py")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spmsg_ac07() {
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let before = active_tenants(&server);
    let upstream = server.base_url.trim_start_matches("http://").to_string();
    let (addr, seen) = mint_hook(upstream).await;

    let scratch = crate::common::TempDataDir::new();
    std::fs::create_dir_all(&scratch.0).expect("scratch dir");
    let results = scratch.0.join("results.jsonl");
    let (base, results_arg) = (format!("http://{addr}"), results.display().to_string());
    let out = tokio::task::spawn_blocking(move || run_driver(base, results_arg)).await.expect("join");

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(out.status.code(), Some(0), "stdout: {stdout}\nstderr: {stderr}");
    assert_eq!(stdout.lines().last(), Some(PASS), "{stdout}");

    let key = issued_key(&seen);
    let results_text = std::fs::read_to_string(&results).expect("results file");
    assert!(!stdout.contains(&key) && !stderr.contains(&key), "the key reached the driver's output");
    assert!(!results_text.contains(&key), "the key reached the results file");
    let row: serde_json::Value =
        serde_json::from_str(results_text.lines().last().expect("a row")).expect("row is json");
    assert_eq!(row["summary"], PASS, "{row}");
    assert_eq!(row["result"], "pass", "{row}");
    let message = row["message"].as_str().expect("the refusal message is recorded");
    assert!(message.starts_with("invalid spec: ") && message.contains("; minimal spec: "), "{message}");

    assert_eq!(active_tenants(&server), before, "the driver's tenant is offboarded");
}
