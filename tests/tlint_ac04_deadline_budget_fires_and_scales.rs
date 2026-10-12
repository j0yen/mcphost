//! PRD-mcphost-timing-assertion-lint
//! AC4 (P0) — Given a `deadline_budget!(100, { body })` whose body sleeps
//! 150 ms, When run with `MCPHOST_PERF_SKIP` unset, Then it panics with
//! `deadline_budget: elapsed=… budget=100ms`; Given `MCPHOST_PERF_SKIP=1`,
//! Then it passes (budget x3) and never prints `perf skipped`.

use crate::common;

use common::EnvGuard;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Duration;

fn sleeps_150ms() {
    crate::deadline_budget!(100, {
        std::thread::sleep(Duration::from_millis(150));
    });
}

fn panic_message(f: impl FnOnce()) -> Option<String> {
    let err = catch_unwind(AssertUnwindSafe(f)).err()?;
    err.downcast_ref::<String>()
        .cloned()
        .or_else(|| err.downcast_ref::<&str>().map(|s| s.to_string()))
}

#[tokio::test]
async fn overrunning_body_panics_with_elapsed_and_budget_when_not_skipping() {
    let _g = EnvGuard::clear("MCPHOST_PERF_SKIP").await;
    let msg = panic_message(sleeps_150ms).expect("150ms body must break a 100ms deadline");
    assert!(
        msg.starts_with("deadline_budget: elapsed=") && msg.ends_with("budget=100ms"),
        "unexpected panic message: {msg}"
    );
}

#[tokio::test]
async fn under_perf_skip_the_budget_is_tripled_and_the_body_still_runs() {
    let _g = EnvGuard::set("MCPHOST_PERF_SKIP", "1").await;
    let out = crate::deadline_budget!(100, {
        std::thread::sleep(Duration::from_millis(150));
        7
    });
    assert!(out == 7, "the body must run (never skipped) and its value pass through");
}

#[tokio::test]
async fn under_perf_skip_a_hang_still_fires_at_triple_budget() {
    let _g = EnvGuard::set("MCPHOST_PERF_SKIP", "1").await;
    let msg = panic_message(|| {
        crate::deadline_budget!(20, {
            std::thread::sleep(Duration::from_millis(120));
        });
    })
    .expect("a hang guard must fire on a loaded host too");
    assert!(msg.ends_with("budget=60ms"), "{msg}");
}

#[test]
fn macro_source_never_prints_perf_skipped() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/common/mod.rs"))
        .expect("read tests/common/mod.rs");
    let start = src.find("macro_rules! deadline_budget").expect("macro defined");
    let body = &src[start..];
    let end = body.find("\n}\n").expect("macro end");
    assert!(
        !body[..end].contains("perf skipped"),
        "deadline_budget! must never print `perf skipped`"
    );
}
