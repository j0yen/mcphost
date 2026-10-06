//! PRD-mcphost-second-session-nudge: the daily sweep that gives a
//! one-and-done tenant one message back. A tenant with a first call, no
//! second session 24h later, and a claimed/verified email gets exactly one
//! `return` email (reusing the ownership-moment mail module, requirement
//! 2); an unclaimed tenant is counted (`nudge_channel = 'none'`) and never
//! re-selected (requirement 3). Started alongside every other background
//! task in `main.rs` (`spawn_sweep_scheduler`), same "own daily loop"
//! convention `statusfeed::spawn_daily_tick` already uses -- but
//! [`sweep`] itself is `pub` and callable directly, so integration tests
//! drive one sweep deterministically instead of waiting on real
//! wall-clock alignment (same convention as `triggers::tick_once`).

use std::time::Duration;

use serde::Serialize;

use crate::email::EmailMessage;
use crate::errors::AppError;
use crate::state::{AppState, now_unix};

/// requirement 7 (P2) default: 24h after `first_call_unix` before a
/// tenant is eligible.
pub const DEFAULT_NUDGE_AFTER_HOURS: i64 = 24;
/// requirement 1 (P0) default sweep hour, UTC.
pub const DEFAULT_SWEEP_HOUR: i64 = 9;
/// requirement 2 (AC5): three failed sends abandon the tenant.
const MAX_NUDGE_ATTEMPTS: i64 = 3;
/// technical considerations: "bounded by `LIMIT 500` per sweep".
const SWEEP_LIMIT: i64 = 500;

/// requirement 7: `$MCPHOST_RETURN_NUDGE_AFTER_HOURS`, default
/// [`DEFAULT_NUDGE_AFTER_HOURS`]; any non-positive or unparseable value
/// falls back to the default rather than selecting every tenant ever.
fn nudge_after_hours_from_env() -> i64 {
    std::env::var("MCPHOST_RETURN_NUDGE_AFTER_HOURS")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|h| *h > 0)
        .unwrap_or(DEFAULT_NUDGE_AFTER_HOURS)
}

/// requirement 7: `$MCPHOST_RETURN_NUDGE=off` disables the send while
/// `sweep` still counts (and marks) every tenant it would otherwise have
/// emailed -- any other value (including unset) leaves sending on.
fn nudge_enabled_from_env() -> bool {
    std::env::var("MCPHOST_RETURN_NUDGE").as_deref() != Ok("off")
}

/// requirement 1: `$MCPHOST_RETURN_SWEEP_HOUR`, default
/// [`DEFAULT_SWEEP_HOUR`]; an out-of-range or unparseable value falls back
/// to the default.
fn sweep_hour_from_env() -> i64 {
    std::env::var("MCPHOST_RETURN_SWEEP_HOUR")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|h| (0..24).contains(h))
        .unwrap_or(DEFAULT_SWEEP_HOUR)
}

/// One sweep's outcome -- `admin.returns`/the background loop both log
/// this rather than just an `Ok(())`, so "the first sweep ran" is
/// observable (migration doc: "cap the first sweep at 500 and log the
/// count").
#[derive(Debug, Clone, Default, Serialize)]
pub struct SweepReport {
    pub selected: i64,
    pub emailed: i64,
    pub none: i64,
    pub failed: i64,
    pub abandoned: i64,
}

/// requirement 1/2/3 (AC1-AC5): one sweep run. Selects up to
/// [`SWEEP_LIMIT`] eligible tenants (`Db::returns_sweep_candidates`) and,
/// for each: an unclaimed tenant (or the send disabled by requirement 7)
/// is recorded as `none`; a claimed one gets exactly one `return` email
/// attempt, recorded `email` on success or `email-failed`/`email-abandoned`
/// on failure (AC5's three-strike rule).
pub async fn sweep(state: &AppState) -> Result<SweepReport, AppError> {
    let now = now_unix();
    let cutoff = now - nudge_after_hours_from_env() * 3600;
    let nudge_enabled = nudge_enabled_from_env();
    let candidates = state.db.returns_sweep_candidates(cutoff, SWEEP_LIMIT).await?;
    let mut report = SweepReport::default();
    for candidate in candidates {
        report.selected += 1;
        let claimed = candidate.owner_verified_at.is_some() && candidate.owner_email.is_some();
        if !claimed || !nudge_enabled {
            state.db.record_nudge_none(candidate.id, now).await?;
            report.none += 1;
            continue;
        }
        match send_return_email(state, &candidate).await {
            Ok(()) => {
                state.db.record_nudge_sent(candidate.id, now).await?;
                report.emailed += 1;
            }
            Err(e) => {
                // AC5: "the key is absent from every log line" -- `e`'s
                // `Display` (from `email::HttpEmailClient::send`/
                // `FakeEmailClient::send`) never embeds the API key, only
                // an HTTP status/provider message, so this is safe to log
                // verbatim.
                tracing::warn!(
                    tenant_id = candidate.id,
                    error = %e,
                    "return nudge email failed"
                );
                let attempts = candidate.nudge_attempts + 1;
                if attempts >= MAX_NUDGE_ATTEMPTS {
                    state.db.record_nudge_abandoned(candidate.id, now, attempts).await?;
                    report.abandoned += 1;
                } else {
                    state.db.record_nudge_failed(candidate.id, attempts).await?;
                    report.failed += 1;
                }
            }
        }
    }
    if report.selected > 0 {
        tracing::info!(
            selected = report.selected,
            emailed = report.emailed,
            none = report.none,
            failed = report.failed,
            abandoned = report.abandoned,
            "return sweep complete"
        );
    }
    Ok(report)
}

/// requirement 2 (AC1): the `return` template -- display name, the
/// `remember` string (`tenant_state::first_contact_summary`'s own
/// `last_note`, or "nothing yet"), published tool count, last call time,
/// and the one URL (`AppState::public_url`, i.e. `$MCPHOST_PUBLIC_URL` --
/// never a per-tenant personal link, per the PRD's own "the one URL from
/// MCPHOST_PUBLIC_URL" wording).
async fn send_return_email(
    state: &AppState,
    candidate: &crate::db::ReturnCandidate,
) -> Result<(), AppError> {
    let to = candidate
        .owner_email
        .clone()
        .ok_or_else(|| AppError::Internal("return nudge: claimed tenant has no owner_email".to_string()))?;
    let (_, last_note) = crate::tenant_state::first_contact_summary(state, candidate.id).await?;
    let remember = last_note.unwrap_or_else(|| "nothing yet".to_string());
    let tool_count = state.db.count_tools(candidate.id).await?;
    let last_call = candidate
        .last_seen_unix
        .map(crate::state::rfc3339_from_unix)
        .unwrap_or_else(|| "never".to_string());
    let from = state.email_config.from.clone().unwrap_or_default();
    let reply_to = crate::email::reply_to_address(&from);
    let display_name = &candidate.display_name;
    let message = EmailMessage {
        to,
        from,
        reply_to,
        subject: format!("{display_name} is still waiting for you at mcphost"),
        text_body: format!(
            "Hi,\n\n\"{display_name}\" made its first call to mcphost and hasn't been back \
             since. Here's what it left:\n\nRemember: {remember}\nTools published: {tool_count}\n\
             Last call: {last_call}\n\nPick back up here:\n\n{url}\n\n— mcphost\nmcphost.dev",
            url = state.public_url,
        ),
    };
    state.email_client.send(&message).await
}

fn next_sweep_tick_unix(now_unix: i64, sweep_hour: i64) -> i64 {
    let days = now_unix.div_euclid(86_400);
    let today_run = days * 86_400 + sweep_hour * 3600;
    if today_run > now_unix {
        today_run
    } else {
        today_run + 86_400
    }
}

fn duration_until_next_sweep(now_unix: i64) -> Duration {
    let next = next_sweep_tick_unix(now_unix, sweep_hour_from_env());
    Duration::from_secs((next - now_unix).max(0) as u64)
}

/// requirement 1: the daily sweep, started once alongside the other
/// background tasks in `main.rs`.
pub fn spawn_sweep_scheduler(state: AppState) -> tokio::task::JoinHandle<()> {
    spawn_sweep_loop(state, None)
}

/// Test-only: same loop [`spawn_sweep_scheduler`] runs at real `serve`
/// startup, but firing every `interval` instead of waiting out the real
/// ~daily cadence -- same convention as `statusfeed::spawn_daily_tick_for_test`.
pub fn spawn_sweep_scheduler_for_test(state: AppState, interval: Duration) -> tokio::task::JoinHandle<()> {
    spawn_sweep_loop(state, Some(interval))
}

fn spawn_sweep_loop(state: AppState, interval_override: Option<Duration>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let sleep_for = interval_override.unwrap_or_else(|| duration_until_next_sweep(now_unix()));
            tokio::time::sleep(sleep_for).await;
            if let Err(e) = sweep(&state).await {
                tracing::warn!(error = %e, "return sweep failed");
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_tick_rolls_to_tomorrow_once_past_the_hour() {
        let day_start = 1_700_000_000i64.div_euclid(86_400) * 86_400;
        let before = day_start + 8 * 3600;
        let after = day_start + 10 * 3600;
        assert_eq!(next_sweep_tick_unix(before, 9), day_start + 9 * 3600);
        assert_eq!(next_sweep_tick_unix(after, 9), day_start + 86_400 + 9 * 3600);
    }
}
