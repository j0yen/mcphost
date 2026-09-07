//! `mcphost billing emit-meter`'s business logic (PRD-mcphost-metered-overage
//! P0): reads pro tenants' unemitted `ok` calls above the `meter_state`
//! high-water mark, groups them per tenant, POSTs one Stripe meter event per
//! group (in chunks of at most [`MAX_EVENTS_PER_REQUEST`]), ledgers each
//! group's span, and advances the high-water mark only once every group in
//! the run has been ledgered. `main.rs`'s `billing emit-meter` subcommand is
//! the only caller in production; every other function here is exercised
//! directly by `tests/metering_ac*.rs` against a real (temp-file) [`Db`] and
//! a [`FakeBillingClient`] -- no network, no HTTP server, per the crate's
//! usual "tests never reach the network" rule.

use std::path::Path;

use serde_json::json;

use crate::billing::{BillingClient, MeterEventRequest};
use crate::db::{Db, MeterGroup};
use crate::errors::AppError;

/// Requirement 51: "Batches cap at 100 events per request."
pub const MAX_EVENTS_PER_REQUEST: usize = 100;

/// One pro tenant's span as ledgered by a single run -- what
/// [`EmitMeterOutcome::groups`] reports back to the caller (and what
/// `tests/metering_ac*.rs` asserts on).
#[derive(Debug, Clone)]
pub struct SentGroup {
    pub tenant_id: i64,
    pub first_call_id: i64,
    pub last_call_id: i64,
    pub count: i64,
    /// `"sent"` the first time this exact span is ledgered, `"replay"` if a
    /// row for the same `(tenant_id, first_call_id, last_call_id)` already
    /// existed (AC5).
    pub mode: &'static str,
}

/// The outcome of one `run_once` (or `run_once_without_state_advance`)
/// invocation -- empty (`groups` empty, everything else zero) when there was
/// nothing pending.
#[derive(Debug, Clone)]
pub struct EmitMeterOutcome {
    pub batch_id: String,
    pub requests_sent: usize,
    pub events_sent: usize,
    pub calls_covered: i64,
    pub groups: Vec<SentGroup>,
}

/// A batch id unique enough for a ledger row and a Stripe dedup namespace
/// (not itself security-sensitive -- it never leaves this host except as a
/// column value): wall-clock seconds plus a random suffix, no `uuid`
/// dependency for something this crate's own `Db::insert_billing_event`
/// convention (a plain `TEXT` column, opaque to every reader) doesn't need
/// to be a real UUID.
fn new_batch_id() -> String {
    let now = crate::state::now_unix();
    let suffix: u32 = rand::random();
    format!("batch-{now}-{suffix:08x}")
}

/// Requirement 51's dedup identifier: stable across a crash-and-rerun of the
/// same unadvanced span (AC5), since it's built only from data already in
/// the `calls` table, never from wall-clock time or a random value.
fn identifier_for(tenant_id: i64, first_call_id: i64, last_call_id: i64) -> String {
    format!("mcphost-{tenant_id}-{first_call_id}-{last_call_id}")
}

/// Send every group in `groups`, in chunks of at most
/// [`MAX_EVENTS_PER_REQUEST`] (AC7), ledgering each group immediately after
/// its chunk's POST succeeds. Returns `(requests_sent, events_sent,
/// calls_covered, groups, new_high_water)`; `new_high_water` is the largest
/// `last_call_id` across every ledgered group (the caller decides whether/
/// when to write it to `meter_state`).
///
/// A chunk's POST failing propagates immediately (AC6): groups already
/// ledgered by an earlier chunk in the same call stay ledgered (a smaller
/// blast radius than losing that progress too), but nothing from the
/// failing chunk (or later ones) is written, and the caller never reaches
/// its own `advance_meter_state` call for spans past the failure.
async fn send_and_ledger(
    db: &Db,
    client: &dyn BillingClient,
    event_name: &str,
    groups: &[MeterGroup],
    batch_id: &str,
) -> Result<(usize, usize, i64, Vec<SentGroup>, i64), AppError> {
    let mut requests_sent = 0usize;
    let mut events_sent = 0usize;
    let mut calls_covered = 0i64;
    let mut sent_groups = Vec::with_capacity(groups.len());
    let mut new_high_water = 0i64;

    for chunk in groups.chunks(MAX_EVENTS_PER_REQUEST) {
        let events: Vec<MeterEventRequest> = chunk
            .iter()
            .map(|g| MeterEventRequest {
                event_name: event_name.to_string(),
                identifier: identifier_for(g.tenant_id, g.first_call_id, g.last_call_id),
                stripe_customer_id: g.stripe_customer_id.clone(),
                value: g.count,
            })
            .collect();
        // AC6: an error here returns before anything in this chunk (or any
        // later chunk) is ledgered or reflected in `new_high_water`.
        client.emit_meter_events(&events).await?;
        requests_sent += 1;
        events_sent += events.len();

        for g in chunk {
            let mode = db
                .insert_meter_event_ledger_detecting_replay(
                    batch_id.to_string(),
                    g.tenant_id,
                    g.first_call_id,
                    g.last_call_id,
                    g.count,
                )
                .await?;
            calls_covered += g.count;
            new_high_water = new_high_water.max(g.last_call_id);
            sent_groups.push(SentGroup {
                tenant_id: g.tenant_id,
                first_call_id: g.first_call_id,
                last_call_id: g.last_call_id,
                count: g.count,
                mode,
            });
        }
    }
    Ok((requests_sent, events_sent, calls_covered, sent_groups, new_high_water))
}

/// The production path: read the pending span, send + ledger it, then
/// advance `meter_state.last_call_id` to cover everything just ledgered
/// (requirement 51: "advances `last_call_id` only after a 2xx"). An empty
/// pending span is a no-op success, not an error.
pub async fn run_once(
    db: &Db,
    client: &dyn BillingClient,
    event_name: &str,
) -> Result<EmitMeterOutcome, AppError> {
    let (last_call_id, _updated_at) = db.get_meter_state().await?;
    let groups = db.pending_meter_groups(last_call_id).await?;
    let batch_id = new_batch_id();
    if groups.is_empty() {
        return Ok(EmitMeterOutcome {
            batch_id,
            requests_sent: 0,
            events_sent: 0,
            calls_covered: 0,
            groups: Vec::new(),
        });
    }
    let (requests_sent, events_sent, calls_covered, groups, new_high_water) =
        send_and_ledger(db, client, event_name, &groups, &batch_id).await?;
    db.advance_meter_state(new_high_water.max(last_call_id)).await?;
    Ok(EmitMeterOutcome {
        batch_id,
        requests_sent,
        events_sent,
        calls_covered,
        groups,
    })
}

/// Test-only: everything [`run_once`] does *except* the final
/// `advance_meter_state` call -- simulates a process crash landing between a
/// batch's POST/ledger writes and the high-water-mark update (AC5). A
/// subsequent real [`run_once`] call recomputes the identical (still
/// unadvanced) span, re-POSTs it with the identical identifier, and this
/// time finds the earlier ledger row -- reporting `mode: "replay"` -- before
/// finally advancing the state the "crashed" run never reached.
#[cfg(any(test, feature = "test-support"))]
pub async fn run_once_without_state_advance(
    db: &Db,
    client: &dyn BillingClient,
    event_name: &str,
) -> Result<EmitMeterOutcome, AppError> {
    let (last_call_id, _updated_at) = db.get_meter_state().await?;
    let groups = db.pending_meter_groups(last_call_id).await?;
    let batch_id = new_batch_id();
    if groups.is_empty() {
        return Ok(EmitMeterOutcome {
            batch_id,
            requests_sent: 0,
            events_sent: 0,
            calls_covered: 0,
            groups: Vec::new(),
        });
    }
    let (requests_sent, events_sent, calls_covered, groups, _new_high_water) =
        send_and_ledger(db, client, event_name, &groups, &batch_id).await?;
    Ok(EmitMeterOutcome {
        batch_id,
        requests_sent,
        events_sent,
        calls_covered,
        groups,
    })
}

/// Holds an exclusive, non-blocking `flock` on `<data_dir>/.meter-emit.lock`
/// for as long as it's alive -- releasing the lock (a plain `close(2)` of
/// the underlying fd) happens automatically on `Drop`. Requirement 51: "the
/// subcommand takes a file lock on the data dir so a timer overlap cannot
/// double-read a span."
pub struct MeterLock {
    _file: std::fs::File,
}

/// Acquire [`MeterLock`], or a `meter_emit_locked` [`AppError`] if another
/// `emit-meter` run already holds it (a timer overlap: the previous
/// five-minute run is still in flight).
pub fn acquire_lock(data_dir: &Path) -> Result<MeterLock, AppError> {
    std::fs::create_dir_all(data_dir)
        .map_err(|e| AppError::Storage(format!("meter lock directory: {e}")))?;
    let path = data_dir.join(".meter-emit.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|e| AppError::Storage(format!("meter lock file: {e}")))?;
    let fd = {
        use std::os::fd::AsRawFd;
        file.as_raw_fd()
    };
    // SAFETY: `fd` is a valid, open file descriptor owned by `file` for the duration of this call; `flock` touches no memory through a pointer, acting solely through the kernel's lock table via this fd-only syscall.
    let rc = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        return Err(AppError::Structured {
            code: "meter_emit_locked",
            message: "another `mcphost billing emit-meter` run already holds the lock".to_string(),
            data: json!({}),
        });
    }
    Ok(MeterLock { _file: file })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifier_is_stable_for_the_same_span() {
        assert_eq!(identifier_for(7, 100, 199), "mcphost-7-100-199");
        assert_eq!(
            identifier_for(7, 100, 199),
            identifier_for(7, 100, 199),
            "must be pure -- same inputs, same identifier, every time"
        );
    }

    #[test]
    fn identifier_differs_by_tenant_or_span() {
        assert_ne!(identifier_for(7, 100, 199), identifier_for(8, 100, 199));
        assert_ne!(identifier_for(7, 100, 199), identifier_for(7, 100, 200));
    }

    #[test]
    fn batch_ids_are_unique_across_calls() {
        let a = new_batch_id();
        let b = new_batch_id();
        assert_ne!(a, b);
    }
}
