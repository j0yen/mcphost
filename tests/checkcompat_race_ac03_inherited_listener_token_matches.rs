//! PRD-mcphost-checkcompat-port-race AC3 (P0) — Given a real previous
//! mcphost binary, When it is spawned with the inherited listener and the
//! token env, Then `/healthz` carries `X-Mcphost-Compat-Token` matching
//! and `check_compat` reports ready.
//!
//! Drives the pieces `compat_check::run` itself calls
//! (`bind_loopback_listener`, `spawn_previous`, `wait_ready`) directly via
//! `compat_check::test_support`, the same real `CARGO_BIN_EXE_mcphost`
//! binary as its own "previous" release `tests/checkcompat_ac02_ac03.rs`
//! uses -- but unlike that file (which only observes the CLI's exit code),
//! this test needs to inspect the `/healthz` response's headers directly
//! while the child is still up, which the full `mcphost migrate
//! --check-compat` subprocess doesn't expose.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn mcphost_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_mcphost"))
}

fn scratch_data_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-checkcompat-race-{tag}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("create scratch data dir");
    dir
}

#[tokio::test]
async fn real_previous_binary_answers_healthz_with_matching_token() {
    let data_dir = scratch_data_dir("ac03");
    // `mcphost serve` expects an already-migrated database; `mcphost
    // migrate` (no --check-compat) is exactly the seed step
    // `checkcompat_ac02_ac03.rs`'s `seed_live_db` also uses.
    let status = std::process::Command::new(mcphost_bin())
        .arg("migrate")
        .env("MCPHOST_DATA_DIR", &data_dir)
        .status()
        .expect("run mcphost migrate to seed the data dir");
    assert!(status.success(), "seeding migrate should succeed");

    let listener = mcphost::compat_check::test_support::bind_loopback_listener()
        .expect("bind loopback listener");
    let port = listener.local_addr().expect("listener local_addr").port();
    let token = mcphost::compat_check::test_support::generate_compat_token();

    let mut child = mcphost::compat_check::test_support::spawn_previous_inherited(
        &mcphost_bin(),
        &data_dir,
        listener,
        &token,
    )
    .expect("spawn the previous release with the inherited listener");

    let base_url = format!("http://127.0.0.1:{port}");

    // "check_compat reports ready".
    mcphost::compat_check::test_support::wait_ready(&base_url, port, &token, &mut child)
        .await
        .expect(
            "wait_ready should report ready against a real previous release carrying the matching token",
        );

    // "/healthz carries X-Mcphost-Compat-Token matching".
    let resp = reqwest::Client::new()
        .get(format!("{base_url}/healthz"))
        .send()
        .await
        .expect("GET /healthz");
    let header = resp
        .headers()
        .get("X-Mcphost-Compat-Token")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    assert_eq!(
        header.as_deref(),
        Some(token.as_str()),
        "healthz response must carry our own compat token back"
    );

    let _ = child.start_kill();
    let _ = child.wait().await;
    let _ = std::fs::remove_dir_all(&data_dir);
}
