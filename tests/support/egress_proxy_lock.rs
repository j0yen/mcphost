//! Serializes the
//! `tests/mcphost_sandbox_egress_allowlist_ac{03,04,06,07}_*.rs` files, the
//! only ones that mutate the process-wide `$MCPHOST_EGRESS_PROXY` env var
//! (PRD-mcphost-sandbox-egress-allowlist).
//!
//! Not a crate module: each `tests/*.rs` file is its own crate root, so
//! this is pulled in per-file with `#[path = "support/egress_proxy_lock.rs"]
//! mod egress_proxy_lock;`, same convention as `tests/support/host.rs`.
//!
//! `cargo test`'s default (no nextest) model runs every `#[tokio::test]` fn
//! in a `tests/suite_*.rs` binary concurrently, as threads within ONE
//! process -- observed as a real flake, not a theoretical one: AC6 setting
//! `MCPHOST_EGRESS_PROXY` for its own upgrade-then-succeed step raced AC3's
//! (or AC7's) `remove_var` on the same process-global variable, so AC6's
//! final call saw no proxy and failed with `egress_unavailable`. A held
//! `tokio::sync::Mutex` guard for each such test's whole body serializes
//! just these three against each other (a plain `std::sync::Mutex` guard
//! held across the many `.await` points these tests have would itself be
//! the `clippy::await_holding_lock` anti-pattern this avoids).
static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub async fn guard() -> tokio::sync::MutexGuard<'static, ()> {
    LOCK.lock().await
}
