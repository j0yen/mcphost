//! PRD-mcphost-drift-review requirement 1: the three hooks that record a
//! `context_versions` row -- `host.table.model_set` (table notes), the
//! table-model tick (table schema/role recomputes), and `doc_put` (document
//! versions) -- and requirement 3: each one enqueues the affected questions
//! for re-run in the same call.

use serde_json::{Value, json};

use crate::errors::AppError;
use crate::state::AppState;

use super::queue;

pub(crate) const KIND_TABLE_NOTE: &str = "table_note";
pub(crate) const KIND_TABLE_SCHEMA: &str = "table_schema";
pub(crate) const KIND_DOCUMENT: &str = "document";

/// requirement 1/3: records one `context_versions` row and enqueues it for
/// re-run -- the one write path every hook below funnels through, so a
/// change is never versioned without also being queued (or vice versa).
#[allow(clippy::too_many_arguments)]
async fn record_change(
    state: &AppState,
    tenant_id: i64,
    kind: &str,
    target: &str,
    version: i64,
    definition_json: String,
    actor: Option<String>,
) -> Result<(), AppError> {
    state
        .db
        .insert_context_version(
            tenant_id,
            kind.to_string(),
            target.to_string(),
            version,
            definition_json.clone(),
            actor.clone(),
        )
        .await?;
    queue::enqueue_for_change(state, tenant_id, kind, target, version, actor, &definition_json).await
}

/// `host.table.model_set` (`tables_model::model_set`): every annotation
/// change (table- or column-level `role`/`unit`/`description`/`hidden`) is
/// a "table note" change -- versioned independently of
/// [`record_table_schema_change`] below, with its own per-(tenant, table)
/// version counter (there is no other owning version number to reuse, unlike
/// the table-model tick's own `model_json` version or a document's own
/// `version`).
#[allow(clippy::too_many_arguments)]
pub async fn record_table_note_change(
    state: &AppState,
    tenant_id: i64,
    actor: Option<String>,
    table: &str,
    column: &str,
    key: &str,
    value: &str,
) -> Result<(), AppError> {
    let next_version = state
        .db
        .latest_context_version(tenant_id, KIND_TABLE_NOTE.to_string(), table.to_string())
        .await?
        .unwrap_or(0)
        + 1;
    let definition_json = json!({"column": column, "key": key, "value": value}).to_string();
    record_change(state, tenant_id, KIND_TABLE_NOTE, table, next_version, definition_json, actor).await
}

/// [`crate::tables_model::tick_once`]'s own recompute: every time a stale
/// table model is recomputed into a new version, that recompute is this
/// hook's "table schema" change -- requirement 1's "the table-model tick
/// (role or schema changes)". Reuses the model's own version number rather
/// than a second, independent counter (unlike [`record_table_note_change`],
/// there already is one owning version number here).
pub async fn record_table_schema_change(
    state: &AppState,
    tenant_id: i64,
    table: &str,
    model_version: i64,
    roles: &Value,
) -> Result<(), AppError> {
    let definition_json = roles.to_string();
    record_change(state, tenant_id, KIND_TABLE_SCHEMA, table, model_version, definition_json, None).await
}

/// `doc_put` (`docs::doc_put`): only called when `outcome.changed` is
/// `true` (an identical-content no-op bumps no version, same as
/// `docs.rs`'s own metering gate for it) -- reuses the document's own real
/// `version` number, the same quantity AC6's "document uploaded as version
/// 3" refers to.
#[allow(clippy::too_many_arguments)]
pub async fn record_document_change(
    state: &AppState,
    tenant_id: i64,
    actor: Option<String>,
    name: &str,
    version: i64,
    content_hash: &str,
) -> Result<(), AppError> {
    let definition_json = json!({"content_hash": content_hash, "version": version}).to_string();
    record_change(state, tenant_id, KIND_DOCUMENT, name, version, definition_json, actor).await
}
