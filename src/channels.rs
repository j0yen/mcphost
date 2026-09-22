//! Business logic for `host.channel.open`/`host.channel.post`
//! (PRD-mcphost-agent-mesh-ops): the minimal channel vertical slice this
//! PRD's own AC4 (freeze must cover `host.channel.post`) and AC7 (a
//! deleted tenant's opened channel, post and cursor must leave no trace)
//! need to exist against -- no dependency PRD has built `host.channel.*`
//! yet (see this PRD's own iter_log operator note). Same module split as
//! `messaging.rs`: pure `AppState` + arguments in, `serde_json::Value` (or
//! [`AppError`]) out; storage lives in `db.rs`'s `channels`/`channel_posts`/
//! `channel_cursors` (migration 0026).

use serde_json::{Value, json};

use crate::db::Tenant;
use crate::errors::AppError;
use crate::state::AppState;

fn arg_str(args: &Value, name: &str) -> Result<String, AppError> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AppError::InvalidArgs(format!("missing required argument '{name}'")))
}

/// `host.channel.open(name)`: creates the channel if it doesn't already
/// exist, else returns the existing one -- idempotent, same "creates or
/// returns" shape as `host.agent.contact_request`'s pending-row reuse.
pub async fn open(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
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
pub async fn post(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    // PRD-mcphost-agent-mesh-ops requirement 4 / AC4: see the mirrored
    // check in `messaging::send`.
    if tenant.mesh_frozen_at.is_some() {
        return Err(AppError::mesh_frozen());
    }
    let channel = arg_str(args, "channel")?;
    let body = arg_str(args, "body")?;
    if body.trim().is_empty() {
        return Err(AppError::InvalidArgs("body: must be non-empty".to_string()));
    }
    let data_json = match args.get("data") {
        None | Some(Value::Null) => None,
        Some(v @ Value::Object(_)) => Some(v.to_string()),
        Some(_) => {
            return Err(AppError::InvalidArgs(
                "data: must be a JSON object".to_string(),
            ));
        }
    };
    let outcome = state.db.channel_post(tenant.clone(), channel, body, data_json).await?;
    Ok(json!({
        "post_id": outcome.id,
        "channel_id": outcome.channel_id,
        "seq": outcome.seq,
        "created_at": outcome.created_at,
    }))
}
