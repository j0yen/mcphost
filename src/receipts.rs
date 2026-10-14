//! PRD-mcphost-scheduled-job-receipts: one receipt per finished trigger-fired
//! run (`schedule`, `event`, `manual`) and `host.schedule_receipts`, the
//! one-call answer to "is my schedule still healthy?".
//!
//! Streak state is derived from the receipt rows on every read, never kept
//! as a counter, so pause/resume and the retention sweep cannot
//! desynchronize it.

use std::collections::HashMap;

use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};

use crate::db::{Db, RunRow, Tenant, TriggerRow};
use crate::errors::AppError;
use crate::state::AppState;

/// R1: an excerpt is at most this many characters, plus one `…` when cut.
pub const EXCERPT_CHARS: usize = 240;
/// R3: consecutive failures that raise the inbox message.
pub const ALERT_STREAK: usize = 3;
/// R2: `host.schedule_receipts`' default page size.
pub const DEFAULT_LIMIT: i64 = 20;
/// R2/R6: the largest `limit` accepted; above it is `args_invalid`.
pub const MAX_LIMIT: i64 = 50;

/// One stored receipt.
#[derive(Debug, Clone)]
pub struct ReceiptRow {
    pub id: i64,
    pub trigger_id: String,
    pub tool_name: String,
    pub run_id: String,
    pub source: String,
    pub started_unix: i64,
    pub duration_ms: i64,
    pub status: String,
    pub error_code: Option<String>,
    pub output_excerpt: String,
    pub cost_calls: i64,
}

impl ReceiptRow {
    fn ok(&self) -> bool {
        self.status == "done"
    }

    fn to_json(&self) -> Value {
        json!({
            "trigger_id": self.trigger_id,
            "tool": self.tool_name,
            "run_id": self.run_id,
            "source": self.source,
            "started_unix": self.started_unix,
            "duration_ms": self.duration_ms,
            "status": self.status,
            "error_code": self.error_code,
            "output_excerpt": self.output_excerpt,
            "cost_calls": self.cost_calls,
        })
    }
}

const RECEIPT_COLUMNS: &str = "id, trigger_id, tool_name, run_id, source, started_unix, duration_ms, \
     status, error_code, output_excerpt, cost_calls";

fn receipt_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ReceiptRow> {
    Ok(ReceiptRow {
        id: r.get(0)?,
        trigger_id: r.get(1)?,
        tool_name: r.get(2)?,
        run_id: r.get(3)?,
        source: r.get(4)?,
        started_unix: r.get(5)?,
        duration_ms: r.get(6)?,
        status: r.get(7)?,
        error_code: r.get(8)?,
        output_excerpt: r.get(9)?,
        cost_calls: r.get(10)?,
    })
}

impl Db {
    /// Inserts one receipt; returns its id.
    pub async fn insert_trigger_receipt(&self, tenant_id: i64, row: ReceiptRow) -> Result<i64, AppError> {
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO trigger_receipts (tenant_id, trigger_id, tool_name, run_id, source, \
                 started_unix, duration_ms, status, error_code, output_excerpt, cost_calls) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    tenant_id,
                    row.trigger_id,
                    row.tool_name,
                    row.run_id,
                    row.source,
                    row.started_unix,
                    row.duration_ms,
                    row.status,
                    row.error_code,
                    row.output_excerpt,
                    row.cost_calls
                ],
            )?;
            Ok(conn.last_insert_rowid())
        })
        .await
    }

    /// Every retained receipt of `trigger_ids` (newest first), optionally
    /// only those started at or after `since_unix`.
    pub async fn trigger_receipts(
        &self,
        tenant_id: i64,
        trigger_ids: Vec<String>,
        since_unix: Option<i64>,
    ) -> Result<Vec<ReceiptRow>, AppError> {
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {RECEIPT_COLUMNS} FROM trigger_receipts \
                 WHERE tenant_id = ?1 AND trigger_id = ?2 AND started_unix >= ?3 ORDER BY id DESC"
            ))?;
            let mut out = Vec::new();
            for id in &trigger_ids {
                let rows = stmt
                    .query_map(params![tenant_id, id, since_unix.unwrap_or(i64::MIN)], receipt_from_row)?
                    .collect::<Result<Vec<_>, _>>()?;
                out.extend(rows);
            }
            out.sort_by_key(|r| std::cmp::Reverse(r.id));
            Ok(out)
        })
        .await
    }

    /// R4: deletes every receipt of a tenant on `plan_name` started before
    /// `cutoff_unix`; returns how many went.
    pub async fn purge_trigger_receipts_for_plan(&self, plan_name: String, cutoff_unix: i64) -> Result<usize, AppError> {
        self.with_conn(move |conn| {
            Ok(conn.execute(
                "DELETE FROM trigger_receipts WHERE started_unix < ?2 \
                 AND tenant_id IN (SELECT id FROM tenants WHERE plan = ?1)",
                params![plan_name, cutoff_unix],
            )?)
        })
        .await
    }

    /// `host.trigger.resume`: the failure streak restarts after the newest
    /// receipt this trigger has right now.
    pub async fn reset_trigger_streak(&self, tenant_id: i64, trigger_id: String) -> Result<(), AppError> {
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO trigger_streak_floor (trigger_id, floor_id) \
                 VALUES (?1, COALESCE((SELECT MAX(id) FROM trigger_receipts \
                                        WHERE tenant_id = ?2 AND trigger_id = ?1), 0)) \
                 ON CONFLICT(trigger_id) DO UPDATE SET floor_id = excluded.floor_id",
                params![trigger_id, tenant_id],
            )?;
            Ok(())
        })
        .await
    }

    /// `trigger_id -> floor receipt id` for every trigger in `trigger_ids`
    /// that was ever resumed.
    pub async fn trigger_streak_floors(
        &self,
        trigger_ids: Vec<String>,
    ) -> Result<std::collections::HashMap<String, i64>, AppError> {
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare("SELECT floor_id FROM trigger_streak_floor WHERE trigger_id = ?1")?;
            let mut out = std::collections::HashMap::new();
            for id in trigger_ids {
                if let Some(floor) = stmt.query_row(params![id], |r| r.get::<_, i64>(0)).optional()? {
                    out.insert(id, floor);
                }
            }
            Ok(out)
        })
        .await
    }
}

/// R1: text of at most [`EXCERPT_CHARS`] characters, `…` appended when cut.
pub fn excerpt_of(text: &str) -> String {
    if text.chars().count() <= EXCERPT_CHARS {
        return text.to_string();
    }
    let mut out: String = text.chars().take(EXCERPT_CHARS).collect();
    out.push('…');
    out
}

/// The redacted excerpt of a run's result (or error text): key names that
/// look like secrets and the tenant's own secret values become `•••`, the
/// same scrubbing the demo page applies.
pub async fn redacted_excerpt(state: &AppState, tenant_id: i64, result: &Value) -> String {
    let secrets = crate::demo_page::tenant_secret_values(state, tenant_id).await;
    let redacted = crate::demo_page::redact(result, &secrets);
    let text = match &redacted {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    excerpt_of(&text)
}

/// Which source a finished run's receipt carries, or `None` when the run is
/// not trigger-fired (or is a `host.trigger.test` dry run).
pub fn receipt_source(run: &RunRow) -> Option<&'static str> {
    if run.trigger_ref.is_none() || run.test {
        return None;
    }
    match run.trigger.as_str() {
        "schedule" if run.manual => Some("manual"),
        "schedule" => Some("schedule"),
        "event" => Some("event"),
        _ => None,
    }
}

/// Consecutive failures at the head of `rows` (newest first), not counting
/// receipts at or before their trigger's resume floor.
fn streak_of(rows: &[ReceiptRow], floors: &HashMap<String, i64>) -> usize {
    rows.iter()
        .filter(|r| r.id > floors.get(&r.trigger_id).copied().unwrap_or(0))
        .take_while(|r| !r.ok())
        .count()
}

/// The per-selection summary R2 names, derived from `rows` (newest first).
pub fn summarize(rows: &[ReceiptRow], floors: &HashMap<String, i64>, next_fire_unix: Option<i64>) -> Value {
    let ok = rows.iter().filter(|r| r.ok()).count();
    let streak_failed = streak_of(rows, floors);
    json!({
        "runs": rows.len(),
        "ok": ok,
        "failed": rows.len() - ok,
        "streak_failed": streak_failed,
        "last_ok_unix": rows.iter().find(|r| r.ok()).map(|r| r.started_unix),
        "last_error": rows.iter().find(|r| !r.ok()).map(|r| r.error_code.clone().unwrap_or_else(|| r.status.clone())),
        "next_fire_unix": next_fire_unix,
    })
}

/// How a trigger-fired run ended, as `runs::run_one_job` knows it.
pub struct RunEnd<'a> {
    pub status: &'a str,
    pub error_code: Option<String>,
    /// The result value (or error text) the excerpt is cut from.
    pub result: &'a Value,
    pub duration_ms: i64,
    pub finished_unix: i64,
}

/// Writes the receipt for a run that just reached a terminal state.
/// Best-effort: a failed insert is logged, never a failed run.
pub async fn record_run_end(state: &AppState, run: &RunRow, end: RunEnd<'_>) {
    let RunEnd { status, error_code, result, duration_ms, finished_unix } = end;
    let Some(source) = receipt_source(run) else { return };
    let Some(trigger_id) = run.trigger_ref.clone() else { return };
    // A trigger removed while its run was in flight leaves nothing to report on.
    match state.db.get_trigger(run.tenant_id, trigger_id.clone()).await {
        Ok(Some(_)) => {}
        _ => return,
    }
    let output_excerpt = redacted_excerpt(state, run.tenant_id, result).await;
    let row = ReceiptRow {
        id: 0,
        trigger_id,
        tool_name: run.tool_name.clone(),
        run_id: run.id.clone(),
        source: source.to_string(),
        started_unix: run.started_unix.unwrap_or(finished_unix - duration_ms / 1000),
        duration_ms,
        status: status.to_string(),
        error_code,
        output_excerpt,
        cost_calls: 1,
    };
    let failed = row.status != "done";
    let (trigger_id, tool, last_error) = (
        row.trigger_id.clone(),
        row.tool_name.clone(),
        row.error_code.clone().unwrap_or_else(|| row.status.clone()),
    );
    if let Err(e) = state.db.insert_trigger_receipt(run.tenant_id, row).await {
        tracing::warn!(error = %e, run_id = %run.id, "failed to write trigger receipt");
        return;
    }
    if failed {
        alert_on_streak(state, run.tenant_id, &trigger_id, &tool, &last_error).await;
    }
}

/// R3: exactly when the trigger's failure streak reaches [`ALERT_STREAK`],
/// one `@host` message lands in the tenant inbox. The streak is re-derived
/// from the receipts, so a fourth failure (streak 4) sends nothing and a
/// success resets the count that the next three failures must reach again.
async fn alert_on_streak(state: &AppState, tenant_id: i64, trigger_id: &str, tool: &str, last_error: &str) {
    let rows = match state.db.trigger_receipts(tenant_id, vec![trigger_id.to_string()], None).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(error = %e, trigger_id, "failed to read receipts for the streak check");
            return;
        }
    };
    let floors = state.db.trigger_streak_floors(vec![trigger_id.to_string()]).await.unwrap_or_default();
    if streak_of(&rows, &floors) != ALERT_STREAK {
        return;
    }
    let data = json!({
        "kind": "schedule.failed_streak",
        "trigger_id": trigger_id,
        "tool": tool,
        "last_error": last_error,
        "receipts_call": format!("host.schedule_receipts {{\"trigger_id\": \"{trigger_id}\"}}"),
    });
    if let Err(e) = state
        .db
        .insert_host_notice(
            tenant_id,
            format!("schedule-receipts-{trigger_id}"),
            "@host".to_string(),
            format!("schedule {tool} failed {ALERT_STREAK}x in a row"),
            data.to_string(),
        )
        .await
    {
        tracing::warn!(error = %e, trigger_id, "schedule failure notice failed");
    }
}

/// R5: `host.trigger.get`'s two extra fields -- the newest receipt (`null`
/// before the first run ends) and the current failure streak.
pub async fn last_receipt_and_streak(state: &AppState, tenant_id: i64, trigger_id: &str) -> (Value, usize) {
    let ids = vec![trigger_id.to_string()];
    let rows = state.db.trigger_receipts(tenant_id, ids.clone(), None).await.unwrap_or_default();
    let floors = state.db.trigger_streak_floors(ids).await.unwrap_or_default();
    (
        rows.first().map(ReceiptRow::to_json).unwrap_or(Value::Null),
        streak_of(&rows, &floors),
    )
}

/// R4: the sweeper's receipt step -- every plan's receipts older than its
/// `receipts_retention_days` are deleted. Called from the housekeeping tick
/// ([`crate::channels::tick_once`]).
pub async fn sweep(state: &AppState) -> Result<(), AppError> {
    let now = crate::state::now_unix();
    for plan in &state.plans.plans {
        let cutoff = now - plan.receipts_retention_days.saturating_mul(86_400);
        state.db.purge_trigger_receipts_for_plan(plan.name.clone(), cutoff).await?;
    }
    Ok(())
}

/// `host.schedule_receipts(trigger_id?|tool?, limit?, since_unix?)`.
pub async fn schedule_receipts(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let limit = match args.get("limit") {
        None | Some(Value::Null) => DEFAULT_LIMIT,
        Some(v) => v.as_i64().filter(|n| (1..=MAX_LIMIT).contains(n)).ok_or_else(|| {
            AppError::InvalidArgs(format!("limit must be an integer from 1 to {MAX_LIMIT} (limit ≤ {MAX_LIMIT})"))
        })?,
    };
    let since_unix = match args.get("since_unix") {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            v.as_i64()
                .ok_or_else(|| AppError::InvalidArgs("since_unix must be an integer".into()))?,
        ),
    };
    let triggers: Vec<TriggerRow> = match args.get("trigger_id").and_then(Value::as_str) {
        Some(id) => vec![
            state
                .db
                .get_trigger(tenant.id, id.to_string())
                .await?
                .ok_or_else(|| crate::triggers::trigger_not_found(id))?,
        ],
        None => {
            let tool = args.get("tool").and_then(Value::as_str).map(str::to_string);
            state.db.list_triggers(tenant.id, tool).await?
        }
    };
    let next_fire_unix = triggers
        .iter()
        .filter(|t| t.kind == "schedule" && t.enabled)
        .filter_map(|t| t.next_unix)
        .min();
    let ids = triggers.iter().map(|t| t.id.clone()).collect();
    let rows = state.db.trigger_receipts(tenant.id, ids, since_unix).await?;
    let floors = state.db.trigger_streak_floors(triggers.iter().map(|t| t.id.clone()).collect()).await?;
    let summary = summarize(&rows, &floors, next_fire_unix);
    let page: Vec<Value> = rows.iter().take(limit as usize).map(ReceiptRow::to_json).collect();
    Ok(json!({"rows": page, "summary": summary}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: i64, status: &str) -> ReceiptRow {
        ReceiptRow {
            id,
            trigger_id: "t".into(),
            tool_name: "probe".into(),
            run_id: format!("r{id}"),
            source: "schedule".into(),
            started_unix: id,
            duration_ms: 1,
            status: status.into(),
            error_code: (status != "done").then(|| "boom".to_string()),
            output_excerpt: String::new(),
            cost_calls: 1,
        }
    }

    #[test]
    fn excerpt_cuts_at_240_chars_plus_ellipsis() {
        assert_eq!(excerpt_of("short"), "short");
        let cut = excerpt_of(&"x".repeat(1000));
        assert_eq!(cut.chars().count(), EXCERPT_CHARS + 1);
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn streak_counts_failures_since_the_newest_success() {
        let rows = vec![row(5, "error"), row(4, "error"), row(3, "done"), row(2, "done")];
        let s = summarize(&rows, &HashMap::new(), Some(9));
        assert_eq!(s["streak_failed"], 2);
        assert_eq!(s["ok"], 2);
        assert_eq!(s["last_ok_unix"], 3);
        assert_eq!(s["next_fire_unix"], 9);
    }
}
