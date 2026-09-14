//! Business logic for `host.agent.*` / `admin.agent.*` -- card assembly and
//! handle validation. PRD-mcphost-agent-directory: makes every tenant
//! addressable (namespace always; an optional, unique `@handle`) without
//! ever surfacing a key hash, billing field, or call log through any of
//! these tools (requirement 2 -- restated for every read path here).
//!
//! Same split as `control.rs`: pure `AppState` + arguments in,
//! `serde_json::Value` (or [`AppError`]) out. `handler.rs` is the only place
//! that touches `rmcp` wire types.

use serde_json::{Value, json};

use crate::db::{AgentCard, SetProfileOutcome, Tenant};
use crate::errors::AppError;
use crate::state::AppState;

/// Open question ("Reserve handles (`admin`, `host`, `mcphost`, `system`)"),
/// drafted as reserved per the PRD's own resolution.
const RESERVED_HANDLES: &[&str] = &["admin", "host", "mcphost", "system"];
/// Requirement 3: description ≤ 512 bytes.
const MAX_DESCRIPTION_BYTES: usize = 512;
/// Requirement 3: ≤ 16 tags of ≤ 32 bytes each.
const MAX_TAGS: usize = 16;
const MAX_TAG_BYTES: usize = 32;

fn arg_str_opt<'a>(args: &'a Value, name: &str) -> Option<&'a str> {
    args.get(name).and_then(Value::as_str)
}

/// Requirement 3: `^[a-z][a-z0-9_]{2,31}$`, stored lower-case -- so
/// `profile_set(handle: "Indexer")` collides with an existing `@indexer`
/// even though the caller's casing differs (AC3).
fn validate_handle(raw: &str) -> Result<String, AppError> {
    let lower = raw.to_lowercase();
    let mut chars = lower.chars();
    let starts_lower_alpha = matches!(chars.next(), Some(c) if c.is_ascii_lowercase());
    let valid_shape = starts_lower_alpha
        && (3..=32).contains(&lower.len())
        && lower.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if !valid_shape {
        return Err(AppError::InvalidArgs(format!(
            "handle: must match ^[a-z][a-z0-9_]{{2,31}}$; got '{raw}'"
        )));
    }
    if RESERVED_HANDLES.contains(&lower.as_str()) {
        return Err(AppError::handle_reserved(&lower));
    }
    Ok(lower)
}

fn validate_description(raw: &str) -> Result<String, AppError> {
    if raw.len() > MAX_DESCRIPTION_BYTES {
        return Err(AppError::InvalidArgs(format!(
            "description: {} bytes, over the {MAX_DESCRIPTION_BYTES} byte limit",
            raw.len()
        )));
    }
    Ok(raw.to_string())
}

fn validate_tags(raw: &[Value]) -> Result<Vec<String>, AppError> {
    if raw.len() > MAX_TAGS {
        return Err(AppError::InvalidArgs(format!(
            "tags: {} tags, over the {MAX_TAGS} tag limit",
            raw.len()
        )));
    }
    let mut tags = Vec::with_capacity(raw.len());
    for v in raw {
        let s = v
            .as_str()
            .ok_or_else(|| AppError::InvalidArgs("tags: every entry must be a string".to_string()))?;
        if s.len() > MAX_TAG_BYTES {
            return Err(AppError::InvalidArgs(format!(
                "tags: '{s}' is {} bytes, over the {MAX_TAG_BYTES} byte limit",
                s.len()
            )));
        }
        tags.push(s.to_string());
    }
    Ok(tags)
}

fn validate_contact_policy(raw: &str) -> Result<String, AppError> {
    match raw {
        "open" | "contacts" | "closed" => Ok(raw.to_string()),
        other => Err(AppError::InvalidArgs(format!(
            "contact_policy: must be one of open, contacts, closed; got '{other}'"
        ))),
    }
}

fn card_json(card: &AgentCard) -> Value {
    json!({
        "address": card.address,
        "handle": card.handle,
        "display_name": card.display_name,
        "description": card.description,
        "tags": card.tags,
        "contact_policy": card.contact_policy,
        "last_seen": card.last_seen,
        "source_class": card.source_class,
        "synthetic": card.synthetic,
    })
}

/// `host.agent.whoami` (requirement 2 / AC1): the caller's own address --
/// never `key_hash`, `billing_ref`, or `stripe_customer_id`, unlike
/// `host.whoami` (`control::whoami`), which this deliberately does not
/// extend -- a distinct, minimal identity surface for the agent-directory
/// PRD's own goal ("zero setup" addressability).
pub async fn whoami(state: &AppState, tenant: &Tenant) -> Result<Value, AppError> {
    let profile = state.db.agent_profile(tenant.id).await?;
    Ok(json!({
        "address": tenant.namespace,
        "handle": profile.handle,
        "display_name": tenant.display_name,
        "contact_policy": profile.contact_policy,
        "plan": tenant.plan,
    }))
}

/// `host.agent.profile_set(handle?, description?, tags?, contact_policy?)`
/// (requirement 3 / AC3): each argument absent leaves that field
/// unchanged; an explicit JSON `null` for `handle`/`description` clears it.
/// `tags`/`contact_policy` have no "current value" worth clearing to null
/// (an empty list / the `open` default already mean that), so those two
/// only accept omission (unchanged) or a valid value.
pub async fn profile_set(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let handle = match args.get("handle") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(s)) => Some(Some(validate_handle(s)?)),
        Some(_) => return Err(AppError::InvalidArgs("handle: must be a string or null".to_string())),
    };
    let description = match args.get("description") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(s)) => Some(Some(validate_description(s)?)),
        Some(_) => {
            return Err(AppError::InvalidArgs(
                "description: must be a string or null".to_string(),
            ));
        }
    };
    let tags = match args.get("tags") {
        None => None,
        Some(Value::Array(a)) => Some(validate_tags(a)?),
        Some(_) => {
            return Err(AppError::InvalidArgs(
                "tags: must be an array of strings".to_string(),
            ));
        }
    };
    let contact_policy = match args.get("contact_policy") {
        None => None,
        Some(Value::String(s)) => Some(validate_contact_policy(s)?),
        Some(_) => {
            return Err(AppError::InvalidArgs(
                "contact_policy: must be a string".to_string(),
            ));
        }
    };

    match state
        .db
        .set_agent_profile(tenant.id, handle, description, tags, contact_policy)
        .await?
    {
        SetProfileOutcome::HandleTaken => Err(AppError::handle_taken()),
        SetProfileOutcome::Ok(profile) => Ok(json!({
            "address": tenant.namespace,
            "handle": profile.handle,
            "description": profile.description,
            "tags": profile.tags,
            "contact_policy": profile.contact_policy,
        })),
    }
}

/// `host.agent.lookup(address)` (requirement 4 / AC2/AC4): `Db::lookup_agent`
/// already excludes disabled tenants at the SQL level and a deleted tenant
/// simply has no row, so unknown/disabled/deleted all collapse to the same
/// `None` this maps to the same `agent_not_found` (AC4).
pub async fn lookup(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let address = arg_str_opt(args, "address")
        .ok_or_else(|| AppError::InvalidArgs("missing required argument 'address'".to_string()))?;
    let card = state
        .db
        .lookup_agent(address.to_string())
        .await?
        .ok_or_else(AppError::agent_not_found)?;
    Ok(card_json(&card))
}

/// `host.agent.search(query?, tag?, limit≤50, cursor?)` (requirement 5 /
/// AC5/AC6): `tag` exact match; `query` case-insensitive substring of
/// handle, display name, or description. Filtering and offset pagination
/// happen in Rust over `Db::list_agent_cards`'s already-ordered,
/// already-disabled-excluded rows -- see that method's own doc comment for
/// why (search carries no latency AC to justify indexing this).
///
/// `cursor` is the opaque decimal offset into the filtered result set that
/// the previous page stopped at -- correct for AC6's "two pages disjoint,
/// ordered, together cover the whole set" over a stable snapshot between
/// calls, not resilient to a handle claimed or released between pages
/// shifting the ordering (a stronger keyset cursor is future work if that
/// ever matters in practice).
pub async fn search(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let query = arg_str_opt(args, "query").map(str::to_lowercase);
    let tag = arg_str_opt(args, "tag");
    let limit: usize = args
        .get("limit")
        .and_then(Value::as_i64)
        .unwrap_or(50)
        .clamp(1, 50) as usize;
    let offset: usize = args
        .get("cursor")
        .and_then(Value::as_str)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);

    let all = state.db.list_agent_cards().await?;
    let matched: Vec<&AgentCard> = all
        .iter()
        .filter(|c| tag.is_none_or(|t| c.tags.iter().any(|x| x == t)))
        .filter(|c| {
            let Some(q) = &query else { return true };
            c.handle.as_deref().is_some_and(|h| h.to_lowercase().contains(q.as_str()))
                || c.display_name.to_lowercase().contains(q.as_str())
                || c.description.as_deref().is_some_and(|d| d.to_lowercase().contains(q.as_str()))
        })
        .collect();

    let page: Vec<Value> = matched.iter().skip(offset).take(limit).map(|c| card_json(c)).collect();
    let next_offset = offset + page.len();
    let cursor = if next_offset < matched.len() {
        Some(next_offset.to_string())
    } else {
        None
    };

    Ok(json!({ "agents": page, "cursor": cursor }))
}

/// `admin.agent.lookup(address)` (P1 requirement 8): like [`lookup`] but on
/// any tenant, disabled included -- the goal is recovering a squatted or
/// abusive name, which needs the tenant visible even while disabled.
pub async fn admin_lookup(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let address = arg_str_opt(args, "address")
        .ok_or_else(|| AppError::InvalidArgs("missing required argument 'address'".to_string()))?;
    let card = state
        .db
        .lookup_agent_admin(address.to_string())
        .await?
        .ok_or_else(AppError::agent_not_found)?;
    Ok(card_json(&card))
}

/// `admin.agent.handle_release(address)` (P1 requirement 8 / AC9): frees a
/// handle so it can be claimed again, regardless of which tenant holds it.
/// Accepts the same `@handle` form `host.agent.lookup` does (a bare handle
/// with no `@` also works, since the stored value never carries one).
pub async fn handle_release(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let address = arg_str_opt(args, "address")
        .ok_or_else(|| AppError::InvalidArgs("missing required argument 'address'".to_string()))?;
    let handle = address.strip_prefix('@').unwrap_or(address).to_lowercase();
    let released = state.db.release_handle(handle.clone()).await?;
    Ok(json!({ "handle": handle, "released": released }))
}
