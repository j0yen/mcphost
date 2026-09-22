//! Business logic for `host.channel.*`.
//!
//! Migration 0027 (PRD-mcphost-agent-mesh-ops) built a minimal, ungated
//! `host.channel.open(name)`/`host.channel.post(channel)` slice against a
//! global, name-keyed `channels` table with no membership concept -- kept
//! working unchanged below (the `else` arm of [`open`]/[`post`] that never
//! touches a group). PRD-mcphost-agent-channels (this module's own PRD) is
//! the group-based channel described in the PRD text: `host.channel.
//! open(group=...)` ties a channel to a group the caller owns
//! (`host.group.*`, `src/sharing.rs`); every current member can
//! `host.channel.post`/`read` it; the owner can `close`/`freeze`/
//! `unfreeze` it. Storage is migration 0028's three additive columns on
//! the existing `channels` table plus the existing (already migration
//! 0027) `channel_posts`/`channel_cursors` tables, reused as-is -- see that
//! migration's own doc comment for why a group channel isn't a second,
//! differently-named table.
//!
//! Same module shape as `messaging.rs`/`sharing.rs`: pure `AppState` +
//! arguments in, `serde_json::Value` (or [`AppError`]) out -- `handler.rs`
//! is the only place that touches `rmcp` wire types.

use serde_json::{Value, json};

use crate::db::{ChannelPostOutcome, GroupChannelCtx, Tenant};
use crate::errors::AppError;
use crate::plans::Plan;
use crate::state::AppState;

fn arg_str(args: &Value, name: &str) -> Result<String, AppError> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AppError::InvalidArgs(format!("missing required argument '{name}'")))
}

fn arg_str_opt(args: &Value, name: &str) -> Option<String> {
    args.get(name).and_then(Value::as_str).map(str::to_string)
}

fn arg_i64_opt(args: &Value, name: &str) -> Option<i64> {
    args.get(name).and_then(Value::as_i64)
}

fn arg_bool(args: &Value, name: &str) -> bool {
    args.get(name).and_then(Value::as_bool).unwrap_or(false)
}

fn plan_for<'a>(state: &'a AppState, tenant: &Tenant) -> Result<&'a Plan, AppError> {
    state
        .plans
        .get(&tenant.plan)
        .ok_or_else(|| AppError::Internal(format!("tenant's plan '{}' is not in the loaded plan catalog", tenant.plan)))
}

fn parse_data_arg(args: &Value) -> Result<Option<String>, AppError> {
    match args.get("data") {
        None | Some(Value::Null) => Ok(None),
        Some(v @ Value::Object(_)) => Ok(Some(v.to_string())),
        Some(_) => Err(AppError::InvalidArgs("data: must be a JSON object".to_string())),
    }
}

/// `host.channel.open(name)`: creates the channel if it doesn't already
/// exist, else returns the existing one -- idempotent, same "creates or
/// returns" shape as `host.agent.contact_request`'s pending-row reuse.
///
/// `host.channel.open(group)` (PRD-mcphost-agent-channels requirement 2 /
/// AC1): one channel per group the caller owns, idempotent the same way;
/// `channels_max` per plan bounds how many a tenant may have open across
/// every group it owns.
pub async fn open(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    if let Some(group) = arg_str_opt(args, "group") {
        let plan = plan_for(state, tenant)?;
        let channel = state
            .db
            .channel_group_open(tenant.id, group.clone(), plan.channels_max)
            .await?;
        return Ok(json!({
            "channel_id": channel.id,
            "group": group,
            "created_at": channel.created_at,
        }));
    }
    let name = arg_str(args, "name")?;
    if name.trim().is_empty() {
        return Err(AppError::InvalidArgs("name: must be non-empty".to_string()));
    }
    let channel = state.db.channel_open(tenant.id, name).await?;
    Ok(json!({
        "channel_id": channel.id,
        "name": channel.name,
        "created_at": channel.created_at,
    }))
}

/// `host.channel.post(channel, body, data?)`: `channel` is either the name
/// passed to `host.channel.open` or the `channel_id` it returned.
///
/// When `channel` is a group channel's id (PRD-mcphost-agent-channels), the
/// group-aware path in [`post_group`] runs instead of the legacy, ungated
/// insert below -- a non-member gets `channel_not_found`, byte-identical to
/// a channel id naming nothing at all (AC2), since [`crate::db::Db::
/// channel_group_lookup`] and the legacy `channel_id = ?1 OR name = ?1`
/// lookup below both answer "not found" the same way for anything that
/// isn't a real group channel id.
pub async fn post(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    // PRD-mcphost-agent-mesh-ops requirement 4 / AC4: checked before
    // anything else runs, same as the mirrored check in `messaging::send`.
    if tenant.mesh_frozen_at.is_some() {
        return Err(AppError::mesh_frozen());
    }
    let channel_ref = arg_str(args, "channel")?;
    let body = arg_str(args, "body")?;
    if body.trim().is_empty() {
        return Err(AppError::InvalidArgs("body: must be non-empty".to_string()));
    }
    let data_json = parse_data_arg(args)?;

    if let Some(ctx) = state.db.channel_group_lookup(channel_ref.clone()).await? {
        return post_group(state, tenant, channel_ref, ctx, body, data_json).await;
    }

    let outcome = state.db.channel_post(tenant.clone(), channel_ref, body, data_json).await?;
    Ok(json!({
        "post_id": outcome.id,
        "channel_id": outcome.channel_id,
        "seq": outcome.seq,
        "created_at": outcome.created_at,
    }))
}

fn post_outcome_json(outcome: &ChannelPostOutcome, from_address: &str) -> Value {
    json!({
        "post_id": outcome.id,
        "channel_id": outcome.channel_id,
        "seq": outcome.seq,
        "from_address": from_address,
        "created_at": outcome.created_at,
    })
}

/// AC1/AC2: `host.channel.post` on a group channel -- membership is
/// resolution-time only (requirement 3: "any current member"; AC5: a
/// removed member's own next post/read fails the same way a stranger's
/// always did).
#[allow(clippy::too_many_arguments)]
async fn post_group(
    state: &AppState,
    tenant: &Tenant,
    channel_id: String,
    ctx: GroupChannelCtx,
    body: String,
    data_json: Option<String>,
) -> Result<Value, AppError> {
    if !state.db.channel_group_is_authorized(&ctx, tenant.id).await? {
        return Err(AppError::channel_not_found());
    }
    // AC9: reads keep working past this point (see `read` above, which
    // never looks at `closed_at`); only posting is refused.
    if ctx.closed_at.is_some() {
        return Err(AppError::channel_closed());
    }
    // P1 requirement 10 / AC10: same "reads unaffected" posture as close.
    if ctx.frozen_at.is_some() {
        return Err(AppError::channel_frozen());
    }
    // AC7: requirement 3's whole-call quota check, before anything is
    // stored -- same "checked, not stored, on rejection" posture
    // `messaging::send`'s own `msgs_per_hour` check uses.
    let plan = plan_for(state, tenant)?;
    let since_ms = crate::state::now_unix_ms() - 3_600_000;
    let used = state.db.count_channel_posts_since(tenant.id, since_ms).await?;
    if used >= plan.channel_posts_per_hour {
        return Err(AppError::channel_quota_exceeded(
            "channel_posts_per_hour",
            plan.channel_posts_per_hour,
        ));
    }
    let outcome = state
        .db
        .channel_group_post_insert(tenant.clone(), channel_id.clone(), body.clone(), data_json.clone())
        .await?;
    // PRD-mcphost-agent-wake requirement 4: strictly after the post's own
    // insert has committed above, never inside that transaction -- same
    // ordering `messaging::send`/`reply` already use for their own
    // `fire_message_triggers` call.
    fire_channel_message_triggers(state, &channel_id, &ctx, tenant, &outcome, &body, &data_json).await;
    Ok(post_outcome_json(&outcome, &tenant.namespace))
}

/// AC6: after a channel post commits, fires every enabled `message`-kind
/// trigger a current member (other than the poster) holds that's bound to
/// this exact `channel_id` -- via the same `hooks::enqueue_with_dedupe`
/// helper `messaging::fire_message_triggers` uses for a DM delivery,
/// deduped on `(trigger_id, message_id)` by passing the post's own id as
/// both `dedupe_key` and `message_id` (requirement 7's "same dedupe on
/// (trigger_id, message_id)"). Never fails the post that reached here --
/// same "logged, not propagated" posture as the DM path.
#[allow(clippy::too_many_arguments)]
async fn fire_channel_message_triggers(
    state: &AppState,
    channel_id: &str,
    ctx: &GroupChannelCtx,
    poster: &Tenant,
    outcome: &ChannelPostOutcome,
    body: &str,
    data_json: &Option<String>,
) {
    let members = match state.db.group_member_ids(ctx.owner_tenant_id, ctx.group_name.clone()).await {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(error = %e, channel_id, "channel trigger member lookup failed");
            return;
        }
    };
    if members.is_empty() {
        return;
    }
    let data = data_json.as_deref().and_then(|s| serde_json::from_str::<Value>(s).ok());
    let envelope = json!({
        "message_id": outcome.id,
        "channel_id": channel_id,
        "seq": outcome.seq,
        "from": poster.namespace,
        "body": body,
        "data": data,
        "created_at": outcome.created_at,
    });
    let args_json = envelope.to_string();

    for member_id in members {
        if member_id == poster.id {
            continue;
        }
        let triggers = match state.db.list_enabled_message_triggers(member_id).await {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(error = %e, tenant_id = member_id, "channel trigger lookup failed");
                continue;
            }
        };
        if triggers.is_empty() {
            continue;
        }
        let member = match state.db.find_tenant_by_id(member_id).await {
            Ok(Some(t)) => t,
            _ => continue,
        };
        for row in triggers {
            let Some(scoped_channel) = crate::triggers::parse_message_trigger_channel_id(&row.config_json)
            else {
                continue;
            };
            if scoped_channel != channel_id {
                continue;
            }
            if let Some(from_scope) = crate::triggers::parse_message_trigger_from(&row.config_json)
                && from_scope != poster.namespace
            {
                continue;
            }
            if let Err(e) = crate::hooks::enqueue_with_dedupe(
                state,
                &member,
                &row,
                crate::hooks::EnqueueSpec {
                    trigger_kind: "message",
                    dedupe_key: Some(outcome.id.as_str()),
                    message_id: Some(outcome.id.as_str()),
                    args_json: args_json.clone(),
                },
            )
            .await
            {
                tracing::warn!(error = %e, trigger_id = %row.id, "channel trigger enqueue failed");
            }
        }
    }
}

/// Owner-only `host.channel.close(channel_id)` (AC9): further posts get
/// `channel_closed`; reads keep working (retention still removes old posts
/// on its own schedule).
pub async fn close(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let channel_id = arg_str(args, "channel_id")?;
    let closed = state.db.channel_group_close(tenant.id, channel_id.clone()).await?;
    if !closed {
        return Err(AppError::channel_not_found());
    }
    Ok(json!({"channel_id": channel_id, "closed": true}))
}

async fn set_frozen(state: &AppState, tenant: &Tenant, args: &Value, frozen: bool) -> Result<Value, AppError> {
    let channel_id = arg_str(args, "channel_id")?;
    let ok = state
        .db
        .channel_group_set_frozen(tenant.id, channel_id.clone(), frozen)
        .await?;
    if !ok {
        return Err(AppError::channel_not_found());
    }
    Ok(json!({"channel_id": channel_id, "frozen": frozen}))
}

/// Owner-only `host.channel.freeze(channel_id)` (P1 requirement 10 /
/// AC10): further posts get `channel_frozen`; reads keep working.
pub async fn freeze(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    set_frozen(state, tenant, args, true).await
}

/// Owner-only `host.channel.unfreeze(channel_id)` (AC10): the next post
/// after this succeeds with the next `seq`.
pub async fn unfreeze(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    set_frozen(state, tenant, args, false).await
}

/// AC8's housekeeping tick: for every group channel, deletes posts older
/// than its OWNER's plan `channel_retention_days` -- exposed `pub` (rather
/// than only reachable through a background loop) so an integration test
/// can drive one tick deterministically, same convention
/// [`crate::triggers::tick_once`]/[`crate::db::Db::prune_once`] already
/// use.
pub async fn tick_once(state: &AppState) -> Result<(), AppError> {
    let channels = state.db.list_group_channels_with_owner_plan().await?;
    let now_ms = crate::state::now_unix_ms();
    for (channel_id, plan_name) in channels {
        let Some(plan) = state.plans.get(&plan_name) else { continue };
        let cutoff_ms = now_ms - plan.channel_retention_days.saturating_mul(86_400_000);
        state.db.delete_channel_posts_older_than(channel_id, cutoff_ms).await?;
    }
    Ok(())
}

/// Same background-task lifetime convention as
/// [`crate::triggers::spawn_scheduler`]/[`crate::retention::spawn_prune_scheduler`]
/// -- started once at real `mcphost serve` startup (`main.rs`); never
/// joined. An hourly cadence matches `channel_posts_per_hour`'s own
/// granularity (P0 requirement 8's retention is measured in days, so an
/// hour of staleness in the worst case is negligible against that).
pub fn spawn_channel_retention(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(3_600)).await;
            if let Err(e) = tick_once(&state).await {
                tracing::warn!(error = %e, "channel retention tick failed");
            }
        }
    })
}

/// `host.channel.read(channel_id, cursor?, limit<=100, ack?)`
/// (PRD-mcphost-agent-channels requirement 4 / AC1, AC3, AC4, AC5, AC8):
/// posts with `seq` greater than `cursor` (default: this member's own
/// stored cursor, or 0 for a first read -- AC8's "starts at the first
/// retained seq" falls out of that for free, since a retention-purged
/// post's `seq` simply no longer exists to match `seq > cursor` against,
/// whichever of the two `cursor` was), in `seq` order, plus `next_cursor`.
/// `ack: true` stores `next_cursor` as this member's new stored cursor.
pub async fn read(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let channel_id = arg_str(args, "channel_id")?;
    let limit = arg_i64_opt(args, "limit").unwrap_or(50).clamp(1, 100);
    let ack = arg_bool(args, "ack");
    let cursor_arg = arg_i64_opt(args, "cursor");

    let ctx = state
        .db
        .channel_group_lookup(channel_id.clone())
        .await?
        .ok_or_else(AppError::channel_not_found)?;
    if !state.db.channel_group_is_authorized(&ctx, tenant.id).await? {
        return Err(AppError::channel_not_found());
    }

    let effective_cursor = match cursor_arg {
        Some(c) => c,
        None => state
            .db
            .channel_cursor_seq(channel_id.clone(), tenant.id)
            .await?
            .unwrap_or(0),
    };
    let rows = state.db.channel_posts_after(channel_id.clone(), effective_cursor, limit).await?;
    let next_cursor = rows.last().map(|r| r.seq).unwrap_or(effective_cursor);
    if ack {
        state.db.channel_cursor_ack(channel_id, tenant.id, next_cursor).await?;
    }
    Ok(json!({
        "posts": rows.iter().map(post_row_json).collect::<Vec<_>>(),
        "next_cursor": next_cursor,
    }))
}

fn post_row_json(row: &crate::db::ChannelPostRow) -> Value {
    json!({
        "post_id": row.id,
        "channel_id": row.channel_id,
        "seq": row.seq,
        "from_address": row.from_address,
        "body": row.body,
        "data": row.data,
        "created_at": row.created_at,
    })
}
