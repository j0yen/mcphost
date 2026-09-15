//! PRD-mcphost-checkcompat-port-race AC2: a foreign HTTP server already
//! answering 200 on the address check-compat is polling must never be
//! mistaken for the previous binary this call spawned -- this is the
//! exact shape of the original race (a parallel test's real mcphost
//! answering for a broken "previous" binary), reproduced deterministically
//! by injecting a fake responder (the AC's own second option) instead of
//! trying to win a real timing race against `bind_loopback_listener`,
//! which this PRD's fix makes un-raceable by construction.
//!
//! Mechanism: `wiremock` binds first and owns the address; `--previous` is
//! pointed at a throwaway shell script that never binds anything and never
//! exits during the probe window (so `wait_ready`'s `try_wait()`
//! short-circuit, AC1's job, never fires here -- this test is purely about
//! the token check). `MCPHOST_BIND` (requirement 4's explicit-address
//! path) is set to the wiremock server's own address so `wait_ready` polls
//! exactly the foreign responder.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn scratch_data_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-checkcompat-race-{tag}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).expect("create scratch data dir");
    dir
}

fn mcphost_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_mcphost"))
}

fn seed_live_db(data_dir: &std::path::Path) {
    let status = Command::new(mcphost_bin())
        .arg("migrate")
        .env("MCPHOST_DATA_DIR", data_dir)
        .status()
        .expect("run mcphost migrate");
    assert!(status.success(), "seeding migrate should succeed");
}

/// A stand-in "previous binary" that ignores its `serve` argv entirely,
/// never binds a port, and outlives the whole 10s readiness window --
/// unlike `/bin/false` (AC1's fixture), this must NOT exit early, so the
/// only thing that can ever answer `/healthz` at the polled address is the
/// wiremock foreign server this test controls.
fn write_never_binds_script(dir: &std::path::Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let script = dir.join("never-binds.sh");
    fs::write(&script, "#!/bin/sh\nexec sleep 30\n").expect("write stand-in script");
    let mut perms = fs::metadata(&script)
        .expect("stat stand-in script")
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script, perms).expect("chmod stand-in script");
    script
}

#[tokio::test]
async fn check_compat_never_trusts_a_foreign_server_without_the_token() {
    let data_dir = scratch_data_dir("ac02race");
    seed_live_db(&data_dir);
    let db_path = data_dir.join("mcphost.db");
    let never_binds = write_never_binds_script(&data_dir);

    // A foreign server, bound and answering BEFORE check-compat even
    // starts -- 200 OK on /healthz, deliberately with no
    // X-Mcphost-Compat-Token header.
    let foreign = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/healthz"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&foreign)
        .await;
    let foreign_addr = foreign.address().to_string();

    let output = Command::new(mcphost_bin())
        .arg("migrate")
        .arg("--check-compat")
        .arg("--previous")
        .arg(&never_binds)
        .arg("--db")
        .arg(&db_path)
        .env("MCPHOST_BIND", &foreign_addr)
        .output()
        .expect("run mcphost migrate --check-compat");

    assert_eq!(
        output.status.code(),
        Some(4),
        "a foreign server with no token must never satisfy readiness; got {:?}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    // The `eprintln!` exit summary goes to stderr, but `tracing::warn!`'s
    // "foreign server on port" line goes wherever `tracing_subscriber::fmt()`
    // defaults to (stdout) -- check both rather than assume.
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("foreign server on port"),
        "output should name the foreign-server condition, got: {combined}"
    );

    let _ = fs::remove_dir_all(&data_dir);
}
