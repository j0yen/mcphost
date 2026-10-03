//! PRD-mcphost-compat-check-unprivileged, test (a): a fake "previous
//! release" that crashes writes a marker to stderr and exits 101 -- the
//! exact shape `sandbox::refuse_to_serve_as_root`'s own panic takes when
//! `check-compat` used to exec the previous release under root's real
//! uid. `wait_ready`'s failure text must surface that stderr, not just
//! the bare exit status, so a real cause (a panic, a permissions error)
//! is visible in the deploy journal instead of only "exited before
//! healthz became ready (101)" -- which is exactly what got misread as a
//! schema incompatibility in production on 2026-10-01/10-03.
//!
//! Drives `spawn_previous`/`wait_ready` directly via
//! `compat_check::test_support`, the same seam
//! `tests/checkcompat_race_ac*.rs` already use, rather than the full
//! `mcphost migrate --check-compat` subprocess -- this test's "previous
//! binary" is a throwaway shell script, not a real `mcphost` binary.

use std::io::Write as _;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn scratch_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-compatfix-{tag}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

/// A fake "previous mcphost release": ignores every argument
/// (`spawn_previous` always runs it as `<bin> serve`), prints a marker to
/// its own stderr, and exits 101 -- the exit code `std::process::exit(101)`
/// would produce, matching `sandbox::refuse_to_serve_as_root`'s own panic
/// (a Rust panic's default exit code is 101).
fn write_fake_previous_release(dir: &std::path::Path) -> PathBuf {
    let path = dir.join("fake-previous-mcphost");
    let mut f = std::fs::File::create(&path).expect("create fake previous release script");
    f.write_all(b"#!/bin/sh\necho boom-stderr-marker 1>&2\nexit 101\n")
        .expect("write fake previous release script");
    drop(f);
    let mut perms = std::fs::metadata(&path)
        .expect("stat fake previous release script")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).expect("chmod +x fake previous release script");
    path
}

#[tokio::test]
async fn previous_release_stderr_is_surfaced_in_the_failure_text() {
    let data_dir = scratch_dir("ac1");
    let bin = write_fake_previous_release(&data_dir);

    let listener =
        mcphost::compat_check::test_support::bind_loopback_listener().expect("bind listener");
    let port = listener.local_addr().expect("listener local_addr").port();
    let token = mcphost::compat_check::test_support::generate_compat_token();

    let (mut child, tail) =
        mcphost::compat_check::test_support::spawn_previous_inherited_with_tail(
            &bin, &data_dir, listener, &token,
        )
        .expect("spawn the fake previous release");

    let base_url = format!("http://127.0.0.1:{port}");
    let err = mcphost::compat_check::test_support::wait_ready_with_tail(
        &base_url, port, &token, &mut child, &tail,
    )
    .await
    .expect_err("the fake previous release exits before healthz ever answers");

    assert_eq!(err.step, "previous-up");
    assert!(
        err.detail.contains("exit status: 101"),
        "failure detail should name the exit status, got: {}",
        err.detail
    );
    assert!(
        err.detail.contains("boom-stderr-marker"),
        "failure detail should surface the child's own stderr, got: {}",
        err.detail
    );

    let _ = child.start_kill();
    let _ = child.wait().await;
    let _ = std::fs::remove_dir_all(&data_dir);
}
