//! PRD-mcphost-drift-review requirement 3: the enqueue/dedupe helper every
//! [`super::versions`] hook calls, plus P1 requirement 8/AC10's
//! `host.drift.check` -- the manual-trigger counterpart that enqueues a
//! re-run for a target with no version change.

use serde_json::{Value, json};

use crate::db::Tenant;
use crate::errors::AppError;
use crate::state::AppState;
use crate::tables;

use super::versions::{KIND_DOCUMENT, KIND_TABLE_SCHEMA};

/// requirement 3's own content-hash dedupe key -- a version-driven change
/// (the only caller of [`enqueue_for_change`]) hashes `definition_json`
/// itself, deliberately not `version`: AC3's "the same change applied
/// twice with identical content" produces two different version numbers
/// but the identical `definition_json`, and must still collapse to one
/// review at process time ([`super::rerun`]'s own `try_emit_review` call
/// against this same hash).
fn content_dedupe_hash(tenant_id: i64, kind: &str, target: &str, definition_json: &str) -> String {
    crate::billing::sha256_hex(format!("{tenant_id}|{kind}|{target}|{definition_json}").as_bytes())
}

/// requirement 3: enqueues one `drift_queue` row for a version-driven
/// change -- [`super::versions::record_change`]'s own second half, so a
/// `context_versions` row is never written without also being queued.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn enqueue_for_change(
    state: &AppState,
    tenant_id: i64,
    kind: &str,
    target: &str,
    version: i64,
    actor: Option<String>,
    definition_json: &str,
) -> Result<(), AppError> {
    let dedupe_hash = content_dedupe_hash(tenant_id, kind, target, definition_json);
    state
        .db
        .enqueue_drift(tenant_id, kind.to_string(), target.to_string(), version, actor, dedupe_hash)
        .await?;
    Ok(())
}

fn target_not_found(target: &str) -> AppError {
    AppError::Structured {
        code: "drift_target_not_found",
        message: format!("'{target}' names no declared table or document for this tenant"),
        data: json!({"target": target}),
    }
}

/// P1 requirement 8/AC10: `host.drift.check {target}` -- enqueues a re-run
/// for `target` at its *current* version (no new `context_versions` row:
/// "without a version change"), so the next tick produces a review with
/// the current deltas even though nothing has actually changed. `target`
/// is resolved to a table first, then a document; a name that's neither
/// fails `drift_target_not_found`. The enqueue's own `dedupe_hash` is a
/// fresh ULID (not content-based like [`enqueue_for_change`]'s) -- a
/// manual check is its own event each time, never deduped against a
/// version-driven review for the same target/version.
pub async fn check(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let target = tables::arg_str(args, "target")?;

    let path = tables::tenant_db_path(state, tenant.id);
    let target_for_conn = target.clone();
    let is_table = tables::with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| {
        Ok(tables::load_schema_sync(conn, &target_for_conn).is_ok())
    })
    .await
    .unwrap_or(false);

    let (kind, trigger_version) = if is_table {
        let version = state
            .db
            .latest_context_version(tenant.id, KIND_TABLE_SCHEMA.to_string(), target.clone())
            .await?
            .unwrap_or(1);
        (KIND_TABLE_SCHEMA, version)
    } else if let Some(doc) = state.db.document_find_by_name(tenant.id, target.clone()).await?
        && doc.deleted_at.is_none()
    {
        (KIND_DOCUMENT, doc.version)
    } else {
        return Err(target_not_found(&target));
    };

    let dedupe_hash = crate::state::new_ulid();
    let queued = state
        .db
        .enqueue_drift(tenant.id, kind.to_string(), target.clone(), trigger_version, None, dedupe_hash)
        .await?;

    Ok(json!({
        "target": target,
        "kind": kind,
        "trigger_version": trigger_version,
        "queued": queued,
    }))
}
