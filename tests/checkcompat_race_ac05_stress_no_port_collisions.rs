//! PRD-mcphost-checkcompat-port-race AC5 (P0) — Given `cargo test --
//! --test-threads=32` running the two `checkcompat_ac02_ac03.rs` tests and
//! 30 dummy tests that each spawn a real mcphost on a fresh listener, When
//! the suite runs 200 times in a loop on RedBaron, Then 0 failures.
//!
//! The original defect (PRD problem statement): more concurrently-running
//! tests each grabbing an ephemeral loopback port meant a higher chance
//! that `checkcompat_ac02_ac03`'s broken-previous-binary case would read a
//! *different* test's real server as its own "previous release" coming
//! up. These 30 tests are exactly that concurrency pressure --
//! `common::TestServer::start()` (the same ephemeral-bind-and-serve helper
//! every other `tests/ac*.rs` file already uses) run 30-wide alongside
//! `checkcompat_ac02_ac03.rs`'s two tests. Requirement 1's inherited-fd
//! design means none of these dummy servers' ports can ever collide with
//! `check_compat`'s chosen port (the whole point: no two listeners are
//! ever "just chosen" and raced), so this suite proves the fix rather than
//! merely re-running the old bug and hoping it doesn't reproduce.
//!
//! This file plus `checkcompat_ac02_ac03.rs` is what
//! `scripts/checkcompat-race-soak.sh` (invoked from the build's own
//! verification, not from `cargo test` itself) loops 200 times with
//! `cargo test checkcompat -- --test-threads=32`, matching every test
//! whose module path contains `checkcompat` (both `checkcompat_ac02_ac03`
//! and every `checkcompat_race_ac*` file) without re-running the other
//! ~390 unrelated test files each iteration.

use crate::common;
use common::TestServer;

macro_rules! dummy_spawns_real_mcphost_on_a_fresh_listener {
    ($name:ident) => {
        #[tokio::test]
        async fn $name() {
            let server = TestServer::start().await;
            let resp = reqwest::Client::new()
                .get(format!("{}/healthz", server.base_url))
                .send()
                .await
                .expect("GET /healthz");
            assert!(
                resp.status().is_success(),
                "dummy mcphost instance should answer /healthz, got {}",
                resp.status()
            );
        }
    };
}

dummy_spawns_real_mcphost_on_a_fresh_listener!(dummy_01);
dummy_spawns_real_mcphost_on_a_fresh_listener!(dummy_02);
dummy_spawns_real_mcphost_on_a_fresh_listener!(dummy_03);
dummy_spawns_real_mcphost_on_a_fresh_listener!(dummy_04);
dummy_spawns_real_mcphost_on_a_fresh_listener!(dummy_05);
dummy_spawns_real_mcphost_on_a_fresh_listener!(dummy_06);
dummy_spawns_real_mcphost_on_a_fresh_listener!(dummy_07);
dummy_spawns_real_mcphost_on_a_fresh_listener!(dummy_08);
dummy_spawns_real_mcphost_on_a_fresh_listener!(dummy_09);
dummy_spawns_real_mcphost_on_a_fresh_listener!(dummy_10);
dummy_spawns_real_mcphost_on_a_fresh_listener!(dummy_11);
dummy_spawns_real_mcphost_on_a_fresh_listener!(dummy_12);
dummy_spawns_real_mcphost_on_a_fresh_listener!(dummy_13);
dummy_spawns_real_mcphost_on_a_fresh_listener!(dummy_14);
dummy_spawns_real_mcphost_on_a_fresh_listener!(dummy_15);
dummy_spawns_real_mcphost_on_a_fresh_listener!(dummy_16);
dummy_spawns_real_mcphost_on_a_fresh_listener!(dummy_17);
dummy_spawns_real_mcphost_on_a_fresh_listener!(dummy_18);
dummy_spawns_real_mcphost_on_a_fresh_listener!(dummy_19);
dummy_spawns_real_mcphost_on_a_fresh_listener!(dummy_20);
dummy_spawns_real_mcphost_on_a_fresh_listener!(dummy_21);
dummy_spawns_real_mcphost_on_a_fresh_listener!(dummy_22);
dummy_spawns_real_mcphost_on_a_fresh_listener!(dummy_23);
dummy_spawns_real_mcphost_on_a_fresh_listener!(dummy_24);
dummy_spawns_real_mcphost_on_a_fresh_listener!(dummy_25);
dummy_spawns_real_mcphost_on_a_fresh_listener!(dummy_26);
dummy_spawns_real_mcphost_on_a_fresh_listener!(dummy_27);
dummy_spawns_real_mcphost_on_a_fresh_listener!(dummy_28);
dummy_spawns_real_mcphost_on_a_fresh_listener!(dummy_29);
dummy_spawns_real_mcphost_on_a_fresh_listener!(dummy_30);
