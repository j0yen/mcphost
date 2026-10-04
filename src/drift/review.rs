//! PRD-mcphost-drift-review requirement 7: `host.drift.reviews`/`.review`/
//! `.resolve` -- also `mcphost.drift.reviews` (P1 requirement 10/AC12),
//! which calls [`reviews`] directly so a sandboxed tool gets back exactly
//! the object the real tool returns.

use serde_json::{Value, json};

use crate::db::{DriftReviewRow, Tenant};
use crate::errors::AppError;
use crate::state::AppState;
use crate::tables::arg_str;

fn not_found(item_id: &str) -> AppError {
    AppError::Structured {
        code: "drift_review_not_found",
        message: format!("no open or resolved review '{item_id}' exists for this tenant"),
        data: json!({"item_id": item_id}),
    }
}

/// requirement 7: a summary entry -- no `deltas` (that's [`review`]'s
/// own, fuller shape).
fn summary_json(row: &DriftReviewRow) -> Value {
    json!({
        "item_id": row.item_id,
        "kind": row.kind,
        "target": row.target,
        "old_version": row.old_version,
        "new_version": row.new_version,
        "actor": row.actor,
        "timestamp": row.created_unix,
        "reason": row.reason,
        "changed_count": row.changed_count,
        "regressed_count": row.regressed_count,
    })
}

const LIMIT_DEFAULT: i64 = 50;
const LIMIT_CAP: i64 = 200;

/// `host.drift.reviews {open_only?, limit?}` / `mcphost.drift.reviews` --
/// newest first, tenant-scoped (AC8).
pub async fn reviews(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let open_only = args.get("open_only").and_then(Value::as_bool).unwrap_or(false);
    let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(LIMIT_DEFAULT).clamp(1, LIMIT_CAP);
    let rows = state.db.list_drift_reviews(tenant.id, open_only, limit).await?;
    Ok(json!({"reviews": rows.iter().map(summary_json).collect::<Vec<_>>()}))
}

/// `host.drift.review {item_id}` -- the full shape, `deltas` included.
pub async fn review(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let item_id = arg_str(args, "item_id")?;
    let row = state
        .db
        .get_drift_review(tenant.id, item_id.clone())
        .await?
        .ok_or_else(|| not_found(&item_id))?;
    let deltas: Value = serde_json::from_str(&row.deltas_json).unwrap_or(Value::Array(Vec::new()));
    let mut out = summary_json(&row);
    if let Some(obj) = out.as_object_mut() {
        obj.insert("deltas".to_string(), deltas);
    }
    Ok(out)
}

/// `host.drift.resolve {item_id, reason}` -- AC7: the item then no longer
/// appears under `open_only: true`, and `review` shows the reason.
pub async fn resolve(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let item_id = arg_str(args, "item_id")?;
    let reason = arg_str(args, "reason")?;
    let resolved = state.db.resolve_drift_review(tenant.id, item_id.clone(), reason).await?;
    if !resolved {
        return Err(not_found(&item_id));
    }
    Ok(json!({"item_id": item_id, "resolved": true}))
}
