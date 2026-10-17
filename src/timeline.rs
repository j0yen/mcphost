//! PRD-mcphost-job-timeline-tool: `host.job.timeline(run_id)` (alias
//! `host.job_timeline`) -- one step-by-step view of a chain or triggered run,
//! built from the `runs` rows the host already keeps and nothing else.
//!
//! Every field of a timeline entry is a `RunRow` column or a named derivation
//! from one; [`TIMELINE_FIELDS`] is that mapping, the one place a field is
//! declared, and [`entry_json`] is the only writer of it.

use serde_json::{Value, json};

use crate::db::{RunRow, Tenant};
use crate::errors::AppError;
use crate::state::AppState;

/// `(field, source)` for every key of a timeline `steps` entry; `source` names
/// the `RunRow` column or the derivation the value comes from.
pub const TIMELINE_FIELDS: &[(&str, &str)] = &[
    ("step", "step_no (1 for a run with no children)"),
    ("tool", "tool_name"),
    ("run_id", "id"),
    ("started_unix", "started_unix"),
    ("finished_unix", "finished_unix"),
    ("duration_ms", "duration_ms"),
    ("status", "status (failed|error|timeout -> error; cancelled -> skipped; queued|in_flight -> running)"),
    ("error_class", "error_class"),
    ("error_family", "error_family(error_class)"),
    ("attempts", "attempt"),
    ("bytes_out", "result_ref.bytes (0 when none)"),
];

/// `(code prefix, family)`: the family an `error_class` belongs to. A code
/// matching no prefix is `other`.
pub const ERROR_FAMILIES: &[(&str, &str)] = &[
    ("upstream_", "upstream"),
    ("compose_", "compose"),
    ("args_", "args"),
    ("tool_", "tool"),
    ("budget_", "budget"),
    ("host_", "host"),
];

/// The family `code` is registered under in [`ERROR_FAMILIES`], else `other`.
pub fn error_family(code: &str) -> &'static str {
    ERROR_FAMILIES.iter().find(|(prefix, _)| code.starts_with(prefix)).map_or("other", |(_, family)| family)
}

/// The `error_class` of a step that started and whose run ended without it
/// finishing (its row is still `in_flight` under a terminal parent).
pub const INTERRUPTED: &str = "interrupted";

/// A row's `status` on the timeline wire: `done`, `error`, `running` or
/// `skipped`. An `in_flight` step is `running` while its run is, else it was
/// cut off with its run and reads as `error`.
fn wire_status(row: &RunRow, run_active: bool) -> &str {
    match row.status.as_str() {
        "failed" | "error" | "timeout" => "error",
        "cancelled" => "skipped",
        "queued" => "running",
        s if s == crate::db::COMPOSED_IN_FLIGHT => {
            if run_active { "running" } else { "error" }
        }
        other => other,
    }
}

fn bytes_out(row: &RunRow) -> i64 {
    row.result_ref
        .as_deref()
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .and_then(|v| v.get("bytes").and_then(Value::as_i64))
        .unwrap_or(0)
}

fn entry_json(row: &RunRow, run_active: bool) -> Value {
    let status = wire_status(row, run_active);
    let error_class = match (&row.error_class, status, row.finished_unix) {
        (None, "error", None) => Some(INTERRUPTED.to_string()),
        (class, _, _) => class.clone(),
    };
    json!({
        "step": row.step_no.unwrap_or(1),
        "tool": row.tool_name,
        "run_id": row.id,
        "started_unix": row.started_unix,
        "finished_unix": row.finished_unix,
        "duration_ms": row.duration_ms,
        "status": status,
        "error_class": error_class,
        "error_family": error_class.as_deref().map(error_family),
        "attempts": row.attempt,
        "bytes_out": bytes_out(row),
    })
}

/// `host.job.timeline(run_id)`.
pub async fn job_timeline(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let run_id = args
        .get("run_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AppError::InvalidArgs("missing required argument 'run_id'".to_string()))?;
    let run = state.db.get_run(run_id.clone(), tenant.id).await?.ok_or_else(|| AppError::Structured {
        code: crate::runs::RUN_NOT_FOUND,
        message: format!("no run '{run_id}' for this tenant"),
        data: json!({"run_id": run_id}),
    })?;
    let children = state.db.get_run_children(tenant.id, run_id).await?;
    let rows: Vec<&RunRow> = if children.is_empty() { vec![&run] } else { children.iter().collect() };
    let running = run.status == "running" || run.status == "queued";
    let steps: Vec<Value> = rows.iter().map(|r| entry_json(r, running)).collect();

    let count = |status: &str| steps.iter().filter(|s| s["status"] == status).count();
    let wall_ms = match (running, run.duration_ms, run.started_unix) {
        (false, Some(ms), _) => ms,
        (_, _, Some(started)) => ((crate::state::now_unix() - started) * 1000).max(0),
        _ => 0,
    };
    let format = match args.get("format").and_then(Value::as_str) {
        None | Some("json") => "json",
        Some("text") => "text",
        Some(other) => {
            return Err(AppError::InvalidArgs(format!("format must be \"json\" or \"text\", got \"{other}\"")));
        }
    };
    let totals = json!({
        "steps": steps.len(),
        "done": count("done"),
        "error": count("error"),
        "running": count("running"),
        "skipped": count("skipped"),
        "bytes_out": rows.iter().map(|r| bytes_out(r)).sum::<i64>(),
        "wall_ms": wall_ms,
    });
    if format == "text" {
        return Ok(json!({"format": "text", "text": render_text(&steps, &totals)}));
    }
    Ok(json!({
        "run": {
            "run_id": run.id,
            "tool": run.tool_name,
            "trigger": run.trigger,
            "status": run.status,
            "error_class": run.error_class,
            "started_unix": run.started_unix,
            "finished_unix": run.finished_unix,
            "duration_ms": run.duration_ms,
        },
        "steps": steps,
        "totals": totals,
    }))
}

/// Most lines `format: "text"` prints, the totals line included.
pub const TEXT_MAX_LINES: usize = 40;

fn cell(v: &Value) -> String {
    match v {
        Value::Null => "-".to_string(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// One line per step (`step  tool  status  duration_ms=N  error_class`) plus a
/// `totals` line, at most [`TEXT_MAX_LINES`] lines in all.
fn render_text(steps: &[Value], totals: &Value) -> String {
    let room = TEXT_MAX_LINES - 1;
    let shown = if steps.len() > room { room - 1 } else { steps.len() };
    let mut lines: Vec<String> = steps[..shown]
        .iter()
        .map(|s| {
            format!(
                "{}  {}  {}  duration_ms={}  {}",
                cell(&s["step"]),
                cell(&s["tool"]),
                cell(&s["status"]),
                cell(&s["duration_ms"]),
                cell(&s["error_class"])
            )
        })
        .collect();
    if shown < steps.len() {
        lines.push(format!("… +{} steps", steps.len() - shown));
    }
    lines.push(format!(
        "totals  steps={} done={} error={} running={} skipped={} bytes_out={} wall_ms={}",
        totals["steps"], totals["done"], totals["error"], totals["running"], totals["skipped"], totals["bytes_out"], totals["wall_ms"]
    ));
    lines.join("\n")
}
