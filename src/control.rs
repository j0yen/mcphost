//! Business logic for `signup` and the `host.*` control plane. Pure
//! `AppState` + arguments in, `serde_json::Value` (or [`AppError`]) out —
//! `handler.rs` is the only place that touches `rmcp` wire types.

use serde_json::{Value, json};

use crate::auth::{generate_handoff_token, generate_key, generate_namespace, hash_key};
use crate::db::{HandoffRedeemOutcome, Tenant};
use crate::errors::AppError;
use crate::state::{
    AppState, CALL_TIMEOUT, HANDOFF_TOKEN_TTL_SECS, MAX_REQUEST_BODY_BYTES, MAX_SPEC_BYTES,
    MAX_TOOL_OUTPUT_BYTES, MAX_TOOLS_PER_TENANT, now_unix, validate_tool_name,
};

fn arg_str(args: &Value, name: &str) -> Result<String, AppError> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AppError::InvalidArgs(format!("missing required argument '{name}'")))
}

fn arg_str_opt(args: &Value, name: &str) -> Option<String> {
    args.get(name).and_then(Value::as_str).map(str::to_string)
}

/// PRD-mcphost-signup-kill-switch-and-source requirement 1 / AC3: `signup`'s
/// optional `source` argument -- absent or JSON `null` reads as `None`
/// (requirement 1: "absent means `signup_source = NULL`"), any other
/// non-string value is rejected the same as a string that fails
/// [`crate::state::is_valid_signup_source`], rather than silently treated
/// as absent -- unlike [`arg_bool`]'s handoff flag, a caller that sent a
/// malformed `source` needs to know it was rejected, not that it was
/// quietly dropped.
fn validate_source(args: &Value) -> Result<Option<String>, AppError> {
    let Some(value) = args.get("source") else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let raw = value
        .as_str()
        .ok_or_else(|| AppError::InvalidArgs("source: must be a string".to_string()))?;
    if crate::state::is_valid_signup_source(raw) {
        Ok(Some(raw.to_string()))
    } else {
        Err(AppError::InvalidArgs(format!(
            "source: must be 1-64 chars matching ^[a-z0-9][a-z0-9._-]*$; got '{raw}'"
        )))
    }
}

/// PRD-mcphost-handoff-token requirement 1: `signup`'s `handoff` argument.
/// Absent, `null`, or any non-`bool` value all read as `false` (the
/// existing raw-key behavior, requirement 5 / AC5 -- an old client that has
/// never heard of this argument must see byte-identical output), so only an
/// explicit `true` opts into handoff mode.
fn arg_bool(args: &Value, name: &str) -> bool {
    args.get(name).and_then(Value::as_bool).unwrap_or(false)
}

/// PRD-mcphost-tool-versions requirement 4/8: `host.tool_rollback`'s
/// `version` and `host.tool_diff`'s `from`/`to` -- required integer
/// arguments, same `args_invalid` shape as [`arg_str`] for a missing one.
fn arg_i64(args: &Value, name: &str) -> Result<i64, AppError> {
    args.get(name)
        .and_then(Value::as_i64)
        .ok_or_else(|| AppError::InvalidArgs(format!("missing required argument '{name}'")))
}

/// PRD-mcphost-synthetic-flag requirement 2: read only at signup, never
/// later. An absent or empty header is silent (this is the overwhelmingly
/// common case -- every real signup) and returns `None`; a present-but-invalid
/// one also returns `None` but logs a warning, since the caller sent a
/// header and it was silently ignored rather than acted on. Never returns
/// `Err` -- an invalid label must never fail the signup it's attached to
/// (requirement 2: "signup proceeds exactly as today").
fn validate_synthetic_header(raw: Option<&str>) -> Option<String> {
    let raw = raw?;
    if raw.is_empty() {
        return None;
    }
    if crate::state::is_valid_synthetic_label(raw) {
        Some(raw.to_string())
    } else {
        tracing::warn!(label = %raw, "invalid x-mcphost-synthetic header on signup; storing null");
        None
    }
}

/// PRD-mcphost-tenant-attribution: everything [`signup`] needs from the
/// HTTP/MCP layer beyond `args`/`source_ip`, bundled into one struct
/// rather than growing the positional parameter list past what a call
/// site can read. `synthetic_header`'s presence (any value, even one that
/// later fails [`is_valid_synthetic_label`]) doubles as requirement 1's
/// "harness marker" signal -- see `state::classify_source_class`'s doc
/// comment for why this crate reuses that header rather than adding a
/// second one.
#[derive(Debug, Clone, Copy, Default)]
pub struct SignupAttribution<'a> {
    pub synthetic_header: Option<&'a str>,
    /// The caller's `clientInfo.name` (requirement 2) -- `handler.rs`
    /// reads it via `RequestContext::client_info`, which is either this
    /// call's own `_meta["io.modelcontextprotocol/clientInfo"]` (stateless
    /// requests -- see `peer_client_info`'s doc comment) or, for a
    /// stateful/legacy peer, whatever `initialize` set.
    pub client_name: Option<&'a str>,
    pub client_version: Option<&'a str>,
    /// The transport's `User-Agent` header, when it exposes one.
    pub user_agent: Option<&'a str>,
}

pub async fn signup(
    state: &AppState,
    args: &Value,
    source_ip: &str,
    attribution: SignupAttribution<'_>,
) -> Result<Value, AppError> {
    // PRD-mcphost-abuse-guard-ban-list requirement 2 / AC1: checked before
    // anything else -- a banned address never consumes a rate-limit slot,
    // never trips the pause/disk-floor checks below, and (the AC's own
    // wording) never gets a `signup_events` row written for it.
    crate::bans::enforce(state, "addr", source_ip).await?;
    // PRD-mcphost-signup-kill-switch-and-source requirement 3 / AC4: checked
    // first, fresh off disk on every call -- an operator's `touch`/`rm`
    // takes effect on the very next signup, no restart. Requirement 4/AC5:
    // this check lives only here, in `signup` -- every authenticated
    // `host.*`/`billing.*` call and every trigger fire never calls this
    // function, so pausing signups can never affect them.
    if let Some(pause) = state.signup_pause.status() {
        return Err(AppError::signup_paused(pause.message, pause.retry_after_secs));
    }
    // PRD-mcphost-data-retention requirement 4 (AC6): same disk-floor
    // refusal `host_tool_call`/`tool_publish` check, before any write.
    if !state.disk_guard.is_ok(state.db.data_dir()) {
        return Err(AppError::disk_floor(
            state.disk_guard.free_bytes(state.db.data_dir()),
            state.disk_guard.floor_bytes(),
        ));
    }
    let display_name = arg_str(args, "name")?;
    // PRD-mcphost-signup-kill-switch-and-source requirement 1 / AC3:
    // validated before the rate-limit admit/tenant insert below, so a
    // rejected `source` never consumes a rate-limit slot or creates a row.
    let signup_source = validate_source(args)?;

    // Requirement 1: `source_class` first (loopback IP or the harness
    // marker header; known-fleet display name, synthorg client name, or
    // now $MCPHOST_FLEET_IPS; else external), then `tenants.synthetic`
    // from it -- the explicit stamp when the header carried a valid one,
    // else `harness:fleet-ip` for a fleet-IP-only match (loop/mcphost-
    // fleet-ips requirement 2), else `harness:unstamped` for every other
    // loopback/fleet path, else `None` for a real (`external`) tenant.
    let explicit_label = validate_synthetic_header(attribution.synthetic_header);
    let harness_marker_present = attribution.synthetic_header.is_some();
    let class = crate::state::classify_source_class(
        source_ip,
        harness_marker_present,
        &display_name,
        attribution.client_name,
        &state.fleet_ips,
    );
    // requirement 2: distinguish *why* `class` came back `Fleet` only to
    // pick the right default label -- a fleet-IP match that isn't also a
    // known display name/synthorg client is exactly the "our own boxes,
    // no header, public IP" case this PRD backfills.
    let fleet_ip_only_match = class == crate::state::SourceClass::Fleet
        && !crate::state::is_known_fleet_display_name(&display_name)
        && !attribution.client_name.is_some_and(crate::state::is_known_synthorg_client);
    let synthetic = if class.is_synthetic() {
        Some(explicit_label.unwrap_or_else(|| {
            if fleet_ip_only_match {
                "harness:fleet-ip".to_string()
            } else {
                "harness:unstamped".to_string()
            }
        }))
    } else {
        None
    };

    // PRD-mcphost-provenance-audit requirements 1/2/5: the write-time
    // origin verdict (and its ip_class triage) every metrics surface now
    // reports, derived through the one shared `state::derive_origin`/
    // `state::classify_ip_class` contract rather than per-call-site logic.
    let (origin, origin_detail) = crate::state::derive_origin(class, synthetic.as_deref());
    let ip_class = crate::state::classify_ip_class(source_ip).to_string();

    // PRD-mcphost-call-limits-honest requirement 4 (AC5): the rate-limit
    // check and the signup-event write are now one atomic DB call
    // (`Db::try_admit_signup`) -- see that method's doc comment for why the
    // old check-then-insert sequence let a concurrent burst overshoot the
    // cap.
    let since = crate::state::now_unix() - crate::state::SIGNUP_RATE_LIMIT_WINDOW_SECS;
    let admitted = state
        .db
        .try_admit_signup(
            source_ip.to_string(),
            since,
            state.signup_rate_limit_per_hour,
            synthetic.clone(),
            attribution.user_agent.map(str::to_string),
            origin.to_string(),
            origin_detail.clone(),
            ip_class,
        )
        .await?;
    if !admitted {
        return Err(AppError::RateLimited);
    }

    let key = generate_key();
    let namespace = generate_namespace();
    let key_hash = hash_key(&key);
    let tenant = state
        .db
        .create_tenant_attributed_with_source(
            display_name,
            namespace.clone(),
            key_hash,
            synthetic,
            Some(class.as_str().to_string()),
            attribution.client_name.map(str::to_string),
            attribution.client_version.map(str::to_string),
            origin.to_string(),
            origin_detail,
            signup_source,
        )
        .await?;

    // PRD-mcphost-human-claim-magic-link requirement 1 / AC1: minted once,
    // right after the tenant row exists, for both branches below -- the
    // claim link is a property of the tenant, not of which signup mode the
    // caller picked.
    let claim_url = crate::claim::issue_claim_token(state, &tenant).await?;

    // PRD-mcphost-handoff-token requirement 1 / AC1: opt-in only -- an
    // absent (or non-true) `handoff` argument is byte-identical to today's
    // response below (requirement 5 / AC5). In handoff mode, `key` never
    // leaves this function: it's AES-256-GCM-encrypted with the same
    // cipher `host.secret_set` uses for tenant secrets (`state.secrets`,
    // never a new key of its own) and stored alongside a single-use,
    // short-lived token; the caller redeems that token via `host.redeem`
    // to get `key` back exactly once.
    if arg_bool(args, "handoff") {
        let token = generate_handoff_token();
        let token_hash = hash_key(&token);
        let (key_enc, key_nonce) = state.secrets.encrypt(&key)?;
        let expires_unix = now_unix() + HANDOFF_TOKEN_TTL_SECS;
        let token_id = state
            .db
            .create_handoff_token(tenant.id, token_hash, key_enc, key_nonce, expires_unix)
            .await?;
        // Requirement 2's "journaled (token id, never values)": neither
        // `token` nor `key` is ever passed to `tracing`.
        tracing::info!(
            tenant = %tenant.namespace,
            token_id,
            expires_in = HANDOFF_TOKEN_TTL_SECS,
            "handoff token issued"
        );
        let mut response = json!({
            "tenant": tenant.namespace,
            "tenant_id": tenant.namespace,
            "handoff_token": token,
            "expires_in": HANDOFF_TOKEN_TTL_SECS,
            "usage": "Call host.redeem with this handoff_token (no Authorization header or \
                tenant_key needed) to receive your tenant key exactly once. The token is \
                single-use and expires in expires_in seconds -- a transcript that captured \
                this response is worthless to anyone who reads it after redemption.",
            "next": "host.redeem",
            // PRD-mcphost-human-claim-magic-link requirement 1 / AC1: present
            // in both signup modes -- the claim link belongs to the tenant,
            // not to whichever way this call chose to hand back the key.
            "claim_url": claim_url,
        });
        // PRD-mcphost-signup-kill-switch-and-source requirement 1 / AC1-2:
        // additive -- present only when the caller passed a valid `source`
        // (requirement 5 / AC5 of PRD-mcphost-handoff-token's own
        // "byte-identical when absent" guarantee stays true either way,
        // since this insert is a no-op for every pre-existing caller that
        // never sends `source`).
        if let Some(source) = &tenant.signup_source
            && let Some(obj) = response.as_object_mut()
        {
            obj.insert("source".to_string(), json!(source));
        }
        return Ok(response);
    }

    let mut response = json!({
        "tenant": tenant.namespace,
        "key": key,
        "namespace": namespace,
        "endpoint": format!("{}/mcp", state.public_url.trim_end_matches('/')),
        "claim_url": claim_url,
        // PRD-mcphost-session-key requirement 3 / AC4: told by the payload
        // it is already reading, not a reconnect instruction it cannot
        // follow (this key never attaches to a connection property; the
        // Claude Agent SDK's mcp_servers config is fixed for the session).
        "usage": "Pass this key as the `tenant_key` argument on every tools/call from here \
            on -- e.g. host.tool_publish, host.tool_call -- no reconnect or \
            Authorization header needed.",
        // P1 requirement 7 / AC7: the very first response points at the
        // shortest path to a working tool, rather than leaving the agent
        // to discover `host.quickstart` on its own.
        "next": "host.quickstart",
    });
    // PRD-mcphost-signup-kill-switch-and-source requirement 1 / AC1-2: the
    // response echoes `source` back only when the caller sent a valid one
    // (AC2: "the response omits ... source" when absent).
    if let Some(source) = &tenant.signup_source
        && let Some(obj) = response.as_object_mut()
    {
        obj.insert("source".to_string(), json!(source));
    }
    Ok(response)
}

/// `host.redeem` (PRD-mcphost-handoff-token requirement 2 / AC2):
/// unauthenticated, like `signup` -- the caller has nothing but the
/// handoff token to offer yet. Exchanges a valid, unexpired,
/// not-yet-redeemed token for the tenant key it was issued for, exactly
/// once; every other outcome is a distinct structured error naming what
/// went wrong without ever echoing the token.
pub async fn redeem(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let token = arg_str(args, "handoff_token")?;
    let token_hash = hash_key(&token);
    match state.db.redeem_handoff_token(token_hash).await? {
        HandoffRedeemOutcome::NotFound => Err(AppError::HandoffTokenInvalid),
        HandoffRedeemOutcome::AlreadyRedeemed { token_id } => {
            tracing::warn!(token_id, "host.redeem refused: token already redeemed");
            Err(AppError::HandoffTokenRedeemed)
        }
        HandoffRedeemOutcome::Expired { token_id } => {
            tracing::warn!(token_id, "host.redeem refused: token expired");
            Err(AppError::HandoffTokenExpired)
        }
        HandoffRedeemOutcome::Redeemed {
            token_id,
            key_enc,
            key_nonce,
            ..
        } => {
            let key = state.secrets.decrypt(&key_enc, &key_nonce)?;
            tracing::info!(token_id, "handoff token redeemed");
            Ok(json!({
                "key": key,
                "usage": "Pass this key as the tenant_key argument on every tools/call from \
                    here on -- e.g. host.tool_publish, host.tool_call -- no reconnect or \
                    Authorization header needed. This token is now dead; redeeming it again \
                    fails with handoff_token_redeemed.",
                "next": "host.quickstart",
            }))
        }
    }
}

/// `host.key_rotate` (PRD-mcphost-handoff-token requirement 3 / AC3):
/// authenticated as the tenant (with its current key, header or
/// `tenant_key` argument -- same as any other `host.*` call). Issues a
/// brand-new key, replaces the stored hash atomically, and returns the new
/// key exactly once; the presented (now former) key stops authenticating
/// anything from this point on, since `resolve_auth`/`resolve_tenant_key_auth`
/// both look a presented key up by hash, and this tenant's row no longer
/// carries the old one.
pub async fn key_rotate(state: &AppState, tenant: &Tenant) -> Result<Value, AppError> {
    let new_key = generate_key();
    let new_key_hash = hash_key(&new_key);
    state.db.rotate_tenant_key(tenant.id, new_key_hash).await?;
    tracing::info!(tenant = %tenant.namespace, "tenant key rotated");
    Ok(json!({
        "tenant": tenant.namespace,
        "key": new_key,
        "usage": "This replaces your previous tenant key immediately -- pass this new key as \
            the tenant_key argument (or Authorization header) on every call from here on; \
            the old key now fails as unauthenticated.",
    }))
}

/// `host.self_offboard()` (PRD-mcphost-tenant-self-offboard P0 requirements
/// 1-2, AC1-4): the tenant's own public path to close its account -- no
/// admin key, no operator ticket. Authenticated exactly like any other
/// `host.*` call (`tenant_key` argument or `Authorization` header, same as
/// `host.key_rotate` above).
///
/// AC2's idempotency needs no double-flip branch here: `resolve_auth`/
/// `resolve_tenant_key_auth` (`handler.rs`) already refuse a *disabled*
/// tenant's key before `dispatch_tenant_tool` ever calls this function --
/// `tenant_disabled` on the header path, `tenant_key_invalid` on the
/// `tenant_key`-argument path (AC3, same shape as an unissued key). So a
/// second `host.self_offboard()` call with an already-offboarded tenant's
/// key never reaches this body at all; it gets the same well-typed,
/// non-crashing refusal every other `host.*`/`billing.*` call already gets
/// from that key, which is precisely AC2's "clean typed result, not a
/// crash or ambiguous error." [`crate::db::Db::self_offboard_tenant`]'s own
/// `WHERE disabled = 0` guard is the second, defense-in-depth layer for
/// that same invariant, not the primary mechanism.
pub async fn self_offboard(state: &AppState, tenant: &Tenant) -> Result<Value, AppError> {
    // AC4: if this is a `pro` tenant with a Stripe customer on file,
    // cancel any live subscription BEFORE flipping the local row -- a
    // crash between the two calls leaves the tenant enabled (retryable),
    // never disabled with billing still running.
    let mut billing_canceled: Vec<String> = Vec::new();
    if tenant.plan == "pro"
        && let Some(stripe_customer_id) = tenant.stripe_customer_id.as_deref()
    {
        billing_canceled = state
            .billing_client
            .cancel_active_subscriptions(stripe_customer_id)
            .await?;
    }

    state.db.self_offboard_tenant(tenant.id).await?;

    // Same retention convention `admin.tenant_disable` already applies
    // (`src/admin.rs::tenant_disable`) -- every registered kind gets a
    // chance to tear down anything it's keeping alive for this tenant
    // (e.g. python's warm sandbox pool). Non-Goals: this does not scrub
    // `tools`/`secrets`/`signup_events` rows -- those stay for audit, same
    // as an admin-disabled tenant today.
    for k in state.kinds.all() {
        k.on_tenant_removed(tenant.id).await;
    }

    tracing::info!(tenant = %tenant.namespace, "tenant self-offboarded");

    Ok(json!({
        "tenant": tenant.namespace,
        "disabled": true,
        "disabled_reason": "self_offboard",
        "billing_canceled": billing_canceled,
    }))
}

/// PRD-mcphost-first-publish-real-kind requirement 1 (AC1): the documented
/// first publish -- a real python tool, not the echo stub. Reverses its
/// input and counts its words, so `host.quickstart`'s own `test_call`
/// (`{"text": "hello"}`) proves the tool actually does something
/// (`{"reversed": "olleh", "words": 1}`) rather than merely publishing.
/// Free-plan-safe by construction: no `secrets`, no `network`, no
/// `requirements`.
const STARTER_TOOL_NAME: &str = "text_stats";
const STARTER_TOOL_SOURCE: &str = "def main(args):\n    text = args.get(\"text\", \"\")\n    reversed_text = text[::-1]\n    words = len(text.split())\n    return {\n        \"reversed\": reversed_text,\n        \"words\": words,\n    }\n";

/// PRD-mcphost-first-publish-real-kind requirement 6 (AC8): `host.quickstart
/// {kind: "http"}`'s own starter -- a public JSON API with no auth, for a
/// tenant that cannot use the python sandbox. `MCPHOST_HTTP_STARTER_URL`
/// overrides the default (test-only in practice today -- a test pointing
/// this at a local fixture upstream is the only real-world reason to set
/// it); production never sets it.
const HTTP_STARTER_TOOL_NAME: &str = "public_fact";
const HTTP_STARTER_DEFAULT_URL: &str = "https://catfact.ninja/fact";

/// Builds the value `quickstart`'s `starter_tool` field carries for
/// `kind == "http"` -- same shape as the python starter (requirement 1),
/// just a wrapped public endpoint instead of a sandboxed script.
fn http_starter_tool() -> Value {
    let url = std::env::var("MCPHOST_HTTP_STARTER_URL")
        .unwrap_or_else(|_| HTTP_STARTER_DEFAULT_URL.to_string());
    let spec = json!({"method": "GET", "url": url});
    json!({
        "name": HTTP_STARTER_TOOL_NAME,
        "kind": "http",
        "spec": spec,
        "publish_call": {
            "call": "host.tool_publish",
            "arguments": {"name": HTTP_STARTER_TOOL_NAME, "kind": "http", "spec": spec},
        },
        "test_call": {
            "call": "host.tool_test",
            "arguments": {"name": HTTP_STARTER_TOOL_NAME, "args": {}},
        },
    })
}

/// `host.quickstart(kind)` (requirement 4, AC3/AC4): the ordered sequence
/// to a working tool of `kind`, with the tenant's own namespace and a
/// filled-in [`crate::kinds::KindExample`] substituted in, plus the current
/// limits. Read-only -- no DB write, ever -- so an agent can call it as
/// many times as it wants while iterating.
///
/// `tenant: None` is the unauthenticated path (AC4): no kind lookup is
/// attempted (there is nothing tenant-specific to fill in yet), and the
/// single step returned is `signup` itself -- no tenant data of any kind
/// is in the response.
///
/// Step order deliberately differs from the PRD's requirement-4 prose
/// (`host.tool_test` -> `host.tool_publish` -> the real call): this crate's
/// `host.tool_test` dry-runs an *already-published* tool (`handler.rs`
/// looks it up by name before calling it), so it cannot run before
/// `host.tool_publish` the way the requirement's ordering implies. The
/// order below -- publish, then test (safe to retry, doesn't count toward
/// `host.usage`/`host.tool_logs`), then the real call -- is the sequence
/// that actually works against this server.
pub fn quickstart(
    state: &AppState,
    tenant: Option<&Tenant>,
    args: &Value,
) -> Result<Value, AppError> {
    let Some(tenant) = tenant else {
        return Ok(json!({
            "authenticated": false,
            "steps": [{
                "call": "signup",
                "arguments": {"name": "<your name>", "handoff": true},
                "note": "Sign up first to get a tenant_key and namespace, then call \
                    host.quickstart again (kind still required) with that key -- as the \
                    tenant_key argument, or reconnected with an Authorization header -- \
                    for a filled-in example. Recommended: signup with handoff: true (shown \
                    above) and call host.redeem once with the returned handoff_token to get \
                    the key, so the signup/redeem transcript carries a dead credential; \
                    omitting handoff returns the raw key directly instead, unchanged from \
                    before. A host.* call with no tenant_key fails with tenant_key_missing; \
                    one that doesn't match any tenant fails with tenant_key_invalid.",
            }],
        }));
    };

    // PRD-mcphost-first-publish-real-kind requirement 1 (AC1): `kind` is now
    // optional -- an agent that just calls `host.quickstart` with no
    // arguments (the documented "First run" flow) gets the python starter
    // recipe below rather than an `args_invalid` rejection.
    let kind_name = arg_str_opt(args, "kind").unwrap_or_else(|| "python".to_string());
    let kind = state
        .kinds
        .get(&kind_name)
        .ok_or_else(|| AppError::UnknownKind {
            requested: kind_name.clone(),
            registered: state.kinds.names(),
        })?;
    let example = kind.example();
    let tool_name = "my_tool";
    let qualified_name = format!("{}.{}", tenant.namespace, tool_name);

    // PRD-grand-loop-billing requirement "host.quickstart's limits object
    // reports the tenant's plan quotas": additive, best-effort -- a plan
    // name plans.toml no longer carries (should never happen; the plan
    // column and the catalog are both this host's own state) degrades to
    // omitting the block rather than failing an otherwise read-only call.
    let plan_limits = state.plans.get(&tenant.plan).map(|plan| {
        json!({
            "name": tenant.plan,
            "tools_max": plan.tools_max,
            "calls_per_day": plan.calls_per_day,
            "secrets_max": plan.secrets_max,
            // PRD-mcphost-tenant-state requirement 6 / AC10.
            "state_bytes_max": plan.state_bytes_max,
            "state_rows_max": plan.state_rows_max,
            "state_ops_per_call_max": plan.state_ops_per_call_max,
            // PRD-mcphost-sharing requirement 4: "host.quickstart limits
            // lists it".
            "shared_tools_max": plan.shared_tools_max,
            // PRD-mcphost-tenant-tables requirement 4: "host.quickstart
            // limits lists them", same shape as state_*_max above.
            "table_tables_max": plan.table_tables_max,
            "table_rows_max": plan.table_rows_max,
            "table_bytes_max": plan.table_bytes_max,
            // PRD-mcphost-runs-and-jobs P0 requirement 8.
            "job_max_s": plan.job_max_s,
            "jobs_concurrent": plan.jobs_concurrent,
            // PRD-mcphost-schedules P0 requirement 4: "host.quickstart
            // limits lists both".
            "schedules_max": plan.schedules_max,
            "schedule_min_interval_s": plan.schedule_min_interval_s,
            // PRD-mcphost-inbound-events requirement 4: "host.quickstart
            // limits lists them".
            "event_triggers_max": plan.event_triggers_max,
            "events_per_minute": plan.events_per_minute,
            "event_body_bytes_max": plan.event_body_bytes_max,
        })
    });
    // PRD-mcphost-call-limits-honest requirement 5 / AC6: the six limits an
    // agent would set or read at call time, read straight from the same
    // constants/catalog entry the enforcement path reads -- never a
    // hand-copied number that can drift from what the code actually does.
    // A tenant's plan gone missing from the catalog (should never happen)
    // degrades to the `free` default, same as `plan_limits` above.
    let concurrent_calls_per_tenant = state
        .plans
        .get(&tenant.plan)
        .map(|plan| plan.concurrent_calls_per_tenant)
        .unwrap_or(4);

    // PRD-mcphost-surface-fluidity requirement 1 (Goal 1, AC1): the one
    // place naming which of the four dry-run tools fits which case, each
    // row with the exact call an agent can make right now -- every dry-run
    // tool's own descriptor (handler.rs's `host_tools`) is now one sentence
    // plus a pointer here instead of re-explaining the other three.
    // `host.bridge_test` is `http`-kind-specific and `host.tool_run` is
    // `python`-kind-specific (see their own descriptions), so those two
    // rows use that kind's own example regardless of the `kind` this call
    // requested; `host.tool_test`/`host.spec_test` work for any kind, so
    // those two use the requested one, same as `steps` above.
    let mut try_before_call = vec![json!({
        "case": "a published tool, by name",
        "call": "host.tool_test",
        "arguments": {"name": tool_name, "args": example.call_args},
    })];
    if let Some(http) = state.kinds.get("http") {
        let http_example = http.example();
        try_before_call.push(json!({
            "case": "an unpublished http spec, against its real upstream",
            "call": "host.bridge_test",
            "arguments": {"spec": http_example.spec, "args": http_example.call_args},
        }));
    }
    try_before_call.push(json!({
        "case": "an unpublished spec of any kind, with example invocations",
        "call": "host.spec_test",
        "arguments": {
            "kind": kind_name,
            "spec": example.spec,
            "invocations": [example.call_args],
        },
    }));
    if let Some(python) = state.kinds.get("python") {
        let python_example = python.example();
        // PRD-mcphost-tool-run-envelope requirement 3 / AC3: the case text
        // now names the standard envelope (`result.payload`) alongside the
        // run metadata, matching `TOOL_RUN_DESC` in `handler.rs`.
        try_before_call.push(json!({
            "case": "a published python tool, for result.payload plus stdout, stderr and exit code",
            "call": "host.tool_run",
            "arguments": {"name": tool_name, "args": python_example.call_args},
        }));
    }

    let steps = vec![
        json!({
            "call": "host.tool_publish",
            "arguments": {"name": tool_name, "kind": kind_name, "spec": example.spec},
            "note": "Publish the tool. A rejection names the field, what was expected, \
                and a corrected example -- fix it and resubmit.",
        }),
        json!({
            "call": "host.tool_test",
            "arguments": {"name": tool_name, "args": example.call_args},
            "note": "Dry-run it: the real call, but it counts toward neither \
                host.usage nor host.tool_logs, so it's safe to repeat while iterating.",
        }),
        json!({
            "call": qualified_name,
            "arguments": example.call_args.clone(),
            "alternative_call": "host.tool_call",
            "alternative_arguments": {"name": tool_name, "args": example.call_args},
            "note": "The real call, either by its namespaced name directly or via \
                host.tool_call by local name -- identical for metering and logs.",
        }),
        json!({
            "call": "host.state.set",
            "arguments": {"key": "example", "value": {"n": 1}},
            // PRD-mcphost-tenant-tables P0 requirement 6 / AC8: one
            // sentence on the table-store vs key-value store choice,
            // placed where an agent actually discovers host.state in
            // the first place.
            "note": "Optional: remember something between calls. host.state.get(key) \
                reads it back; a python tool's own code can read/write the same store. \
                For small unstructured values, host.state stays the right store; for \
                typed rows you'll filter, sort, or aggregate with real SQL, create a \
                table instead with host.table.create/append/query.",
        }),
    ];
    // PRD-mcphost-first-publish-real-kind requirement 1/6 (AC1/AC8): the
    // starter recipe defaults to python (a real tool, not the echo stub)
    // for any request, except `kind: "http"`, which gets its own starter
    // (requirement 6: "for tenants that cannot use the sandbox") --
    // `steps`/`try_before_call` above stay scoped to the requested kind;
    // `starter_tool` is the one, fixed, always-works-on-free recommendation
    // for a tenant with nothing published yet. `next` is the tail of
    // `steps` (the real call, then the optional state-set) -- the actions
    // that come after the starter's own publish+test, which `starter_tool`
    // already carries as `publish_call`/`test_call`.
    let starter_tool = if kind_name == "http" {
        http_starter_tool()
    } else {
        json!({
            "name": STARTER_TOOL_NAME,
            "kind": "python",
            "spec": {"source": STARTER_TOOL_SOURCE},
            "publish_call": {
                "call": "host.tool_publish",
                "arguments": {
                    "name": STARTER_TOOL_NAME,
                    "kind": "python",
                    "spec": {"source": STARTER_TOOL_SOURCE},
                },
            },
            "test_call": {
                "call": "host.tool_test",
                "arguments": {"name": STARTER_TOOL_NAME, "args": {"text": "hello"}},
            },
        })
    };
    let next = steps[2..].to_vec();

    Ok(json!({
        "authenticated": true,
        "namespace": tenant.namespace,
        "kind": kind_name,
        "try_before_call": try_before_call,
        "starter_tool": starter_tool,
        "next": next,
        "steps": steps,
        "limits": {
            "max_spec_bytes": MAX_SPEC_BYTES,
            "max_tools_per_tenant": MAX_TOOLS_PER_TENANT,
            "name_pattern": "^[a-z][a-z0-9_]{1,40}$",
            "plan": plan_limits,
            // Requirement 5 / AC6: named here, and in README/llms.txt, from
            // the exact same constants the enforcement path in
            // `handler.rs`/`kinds::python` reads -- see
            // `tests/limits_ac06_quickstart_docs_match_constants.rs`.
            "call_timeout_default_s": CALL_TIMEOUT.as_secs(),
            "call_timeout_max_s": crate::kinds::python::MAX_TIMEOUT_S,
            "output_bytes_max": MAX_TOOL_OUTPUT_BYTES,
            "request_body_bytes_max": MAX_REQUEST_BODY_BYTES,
            "concurrent_calls_per_tenant": concurrent_calls_per_tenant,
            "concurrent_calls_host": crate::kinds::python::DEFAULT_MAX_CONCURRENT_CALLS,
        },
    }))
}

/// `subject` (PRD-mcphost-oauth-resource-server requirement 4 / AC2): the
/// JWT `sub` claim when this call authenticated via an OAuth bearer,
/// `None` for a key-authenticated caller (header or `tenant_key`
/// argument) -- `caller.subject` is "unused by this PRD beyond logging"
/// per the PRD's own technical considerations, except here, where
/// surfacing it on `host.whoami` is exactly how a caller (or a test)
/// confirms which subject a token resolved to.
pub async fn whoami(state: &AppState, tenant: &Tenant, subject: Option<&str>) -> Result<Value, AppError> {
    // PRD-mcphost-handoff-token P1 requirement 7 / AC8: age is measured
    // from the last rotation when there's been one, else from the
    // tenant's own creation -- a never-rotated key is exactly as old as
    // the tenant. `created_unix` is only `None` for a row this crate never
    // wrote (should not happen post-migration-0010), in which case age is
    // unknowable rather than a misleading guess.
    let key_since = tenant.key_rotated_unix.or(tenant.created_unix);
    let key_age_s = key_since.map(|since| (now_unix() - since).max(0));
    // PRD-mcphost-shared-tool-call-path P1 requirement 6 (AC8): every
    // shared tool this tenant can currently reach, direct (public) or via a
    // group it belongs to -- see `Db::shared_tools_for_caller`'s own doc
    // comment for why "host.get_info" (the PRD's own name for this) landed
    // on `host.whoami`, this crate's real "tell the caller about itself"
    // tool, rather than a new tool of that literal name.
    let shared_tools: Vec<Value> = state
        .db
        .shared_tools_for_caller(tenant.id, tenant.namespace.clone())
        .await?
        .into_iter()
        .map(|(owner_ns, tool_name, via)| {
            json!({
                "name": format!("{owner_ns}.{tool_name}"),
                "owner": owner_ns,
                "via": via,
            })
        })
        .collect();
    Ok(json!({
        "tenant": tenant.namespace,
        "namespace": tenant.namespace,
        "display_name": tenant.display_name,
        "created_at": tenant.created_at,
        "disabled": tenant.disabled,
        // PRD-grand-loop-billing goal: an agent's own identity call
        // already tells it what plan it's on, with no extra round trip.
        "plan": tenant.plan,
        "plan_since": tenant.plan_since,
        // PRD-mcphost-tenant-attribution P1 requirement 6 / AC6: how this
        // host classified the caller, and the client it recorded for it --
        // an agent (or a human testing) can confirm how it was seen
        // without reaching for `admin.tenants`.
        "source_class": tenant.source_class,
        "client_name": tenant.client_name,
        "client_version": tenant.client_version,
        "key_age_s": key_age_s,
        "key_rotated_at": tenant.key_rotated_unix,
        // PRD-mcphost-human-claim-magic-link P1 requirement 8 / AC11: lets
        // the agent tell its human whether the claim actually happened,
        // without ever exposing the email address itself (same
        // bool-only-derived-from-owner_verified_at shape as
        // `admin.tenants`' own `owner_verified`).
        "owner_verified": tenant.owner_verified_at.is_some(),
        // PRD-mcphost-host-tool-deprecation P1 requirement 6 / AC7: the
        // committed contract's own version, so an agent already calling
        // host.whoami for its identity learns which contracts/host-tools.v<N>.json
        // it's coding against with no extra round trip.
        "contract_version": crate::api_contract::CONTRACT_VERSION,
        "subject": subject,
        "shared_tools": shared_tools,
    }))
}

/// PRD-mcphost-admin-schema-contract P2 requirement 7 (AC7): `host.whoami`
/// for the admin key itself, not a tenant -- there is no `Tenant` row to
/// report identity from, so this is a minimal admin-only identity reply
/// naming the one thing an operator holding the admin key needs from it:
/// the schema version `admin.tenants`/`admin.usage` are currently pinned
/// to (`admin::SCHEMA_VERSION`, the same constant those two listings emit).
pub fn whoami_admin() -> Value {
    json!({
        "admin": true,
        "admin_schema_version": crate::admin::SCHEMA_VERSION,
    })
}

/// PRD-mcphost-host-tool-deprecation requirement 4 / AC6: `host.changelog
/// {since?}` -- additions, announced deprecations, and completed removals
/// in the host.*/billing.* surface, derived from the live registry (this
/// tenant's actual `state.kinds`, not the fixed `KindRegistry::with_builtin()`
/// [`crate::api_contract::dump_contract`]'s own committed-contract use
/// picks, since this call answers "what does THIS deployment offer now",
/// same distinction `llms_txt::tenant_tool_names`'s doc already draws for
/// the analogous name-set question) and `state.deprecations`. Pure and
/// synchronous -- no DB read needed beyond what's already in `state`.
pub fn changelog(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let since = args.get("since").and_then(Value::as_str).unwrap_or("0.0.0");
    Ok(crate::api_contract::changelog(
        &state.kinds,
        &state.deprecations,
        since,
    ))
}

/// PRD-mcphost-first-publish-real-kind requirement 4: one entry of
/// `host.tool_publish`'s `gates` array (both the `dry_run: true` response
/// and, on failure, a real publish's own error `data`) -- `{gate, ok,
/// message, fix}`, `message`/`fix` present only when `ok` is `false`.
fn publish_gate(gate: &'static str, ok: bool, message: Option<String>, fix: Option<String>) -> Value {
    json!({"gate": gate, "ok": ok, "message": message, "fix": fix})
}

/// Requirement 4 / AC4: a real (non-`dry_run`) publish that fails one of
/// the "collectible" gates (secrets, env, network, deps) keeps its own
/// original `code`/`message`/`data` (every field an existing test already
/// asserts on) -- this only adds `gates`, so a caller sees every other
/// simultaneously-failing gate too, not just the one this error is named
/// for.
fn attach_gates(err: AppError, gates: &[Value]) -> AppError {
    let code = err.code();
    let message = err.to_string();
    let mut data = match &err {
        AppError::Structured { data, .. } if data.is_object() => data.clone(),
        _ => json!({}),
    };
    if let Value::Object(map) = &mut data {
        map.insert("gates".to_string(), json!(gates));
    }
    AppError::Structured { code, message, data }
}

pub async fn tool_publish(
    state: &AppState,
    tenant: &Tenant,
    args: &Value,
) -> Result<Value, AppError> {
    // PRD-mcphost-data-retention requirement 4 (AC6): same disk-floor
    // refusal `host_tool_call` checks, before any write.
    if !state.disk_guard.is_ok(state.db.data_dir()) {
        return Err(AppError::disk_floor(
            state.disk_guard.free_bytes(state.db.data_dir()),
            state.disk_guard.floor_bytes(),
        ));
    }
    // PRD-mcphost-first-publish-real-kind requirement 4 (AC3/AC4): every
    // gate below this call runs regardless -- `dry_run` just means "collect
    // and report, never write" instead of "stop at the first failure and
    // write once every gate passes."
    let dry_run = arg_bool(args, "dry_run");
    let name = arg_str(args, "name")?;
    let kind_name = arg_str(args, "kind")?;
    let mut spec = args.get("spec").cloned().unwrap_or(Value::Null);

    validate_tool_name(&name)?;

    let spec_bytes = serde_json::to_vec(&spec)
        .map_err(|e| AppError::Internal(format!("spec serialize: {e}")))?
        .len();
    if spec_bytes > MAX_SPEC_BYTES {
        // PRD-mcphost-first-publish-real-kind requirement 4 (AC9): under
        // dry_run, spec_size is reported as a failing gate -- on its own,
        // with no other gate evaluated after it, since a spec too large to
        // safely inspect makes every later gate meaningless. A real publish
        // keeps its original, unchanged `spec_too_large` error.
        if dry_run {
            return Ok(json!({
                "ok": false,
                "gates": [publish_gate(
                    "spec_size",
                    false,
                    Some(format!("spec is {spec_bytes} bytes, over the {MAX_SPEC_BYTES}-byte limit")),
                    Some(format!("shrink the spec under {MAX_SPEC_BYTES} bytes")),
                )],
            }));
        }
        return Err(AppError::SpecTooLarge(spec_bytes));
    }

    let kind = state
        .kinds
        .get(&kind_name)
        .ok_or_else(|| AppError::UnknownKind {
            requested: kind_name.clone(),
            registered: state.kinds.names(),
        })?;

    // PRD-mcphost-tool-kind-honor requirement 1 (AC1/AC2/AC5): an explicit
    // `kind` of `http` or `python` is a constraint checked against what the
    // spec's own shape can only be -- honored (nothing changes below) when
    // they agree, refused with `kind_mismatch` naming the disagreeing
    // element when they don't, before the spec is ever validated against
    // (let alone stored under) the wrong `Kind` impl. Scoped to `http`/
    // `python` (the two kinds this PRD's incident confused); `echo` publishes
    // are unaffected.
    if matches!(kind_name.as_str(), "http" | "python")
        && let Some(signal) = crate::kinds::infer::infer_kind_signal(&spec)
        && signal.kind != kind_name
    {
        return Err(AppError::kind_mismatch(&kind_name, &signal));
    }

    // PRD-mcphost-sandbox-ready requirement 3 (AC3): a kind whose sandbox
    // self-test is currently failing (only `python` reports a status at
    // all -- `sandbox_status()` is `None` for `echo`/`http`, AC4) is
    // rejected here, before `parse_spec`/`ast_check` (inside
    // `validate_all`/`validate_async` below) ever run -- no sandboxed
    // process is spawned for a doomed publish.
    // Non-functional requirement ("dry_run answers ... with no sandbox
    // spin-up"): a `dry_run` never blocks on, or reports, this host's
    // sandbox *readiness* -- unlike a real publish, a busy/broken sandbox
    // mechanism isn't one of `dry_run`'s own named gates (kind, spec size,
    // secrets, env, network, deps, name).
    if !dry_run
        && let Some(status) = kind.sandbox_status()
        && !status.ready
    {
        // PRD-mcphost-first-publish-real-kind requirement 3 (AC2): the
        // kind's own current wait estimate when it has one, else a fixed
        // 5s -- clamped to [1, 30] either way.
        let retry_after_s = kind.queue_wait_estimate_s().unwrap_or(5).clamp(1, 30);
        return Err(AppError::sandbox_unavailable(&status, retry_after_s));
    }

    // Requirement 3 / AC2: every simultaneously-failing field is reported
    // at once, not just the first -- see `AppError::from_kind_violations`.
    if let Some(err) = AppError::from_kind_violations(kind.validate_all(&spec)) {
        return Err(err);
    }
    kind.validate_async(&spec).await?;

    // PRD-mcphost-first-publish-real-kind requirement 4 (AC3/AC4): from here
    // on, every remaining pre-check is "collectible" -- it's recorded as its
    // own `gates` entry and evaluated regardless of whether an earlier one
    // already failed, so a `dry_run` (or a real publish's own rejection)
    // reports every simultaneously-failing gate, not just the first.
    let mut gates: Vec<Value> = Vec::new();

    // gate: secrets -- every `secret.<name>` the spec references must
    // already exist for this tenant. Kinds with no secret-templating
    // concept (`echo`) return no references here, so this is trivially ok
    // for them.
    let referenced = kind.referenced_secrets(&spec);
    let mut missing_secrets: Vec<String> = Vec::new();
    if !referenced.is_empty() {
        let known = state.db.list_secret_names(tenant.id).await?;
        for secret_name in referenced {
            if !known.contains(&secret_name) {
                missing_secrets.push(secret_name);
            }
        }
    }
    gates.push(publish_gate(
        "secrets",
        missing_secrets.is_empty(),
        (!missing_secrets.is_empty())
            .then(|| format!("spec references unknown secret(s): {}", missing_secrets.join(", "))),
        (!missing_secrets.is_empty())
            .then(|| "call host.secret_set for each missing name, then republish".to_string()),
    ));

    // gate: env -- a spec's own `env` names must never collide with a
    // secret name already known for this tenant (PRD-mcphost-python-kind-
    // plain-env requirement 3). The secret store is tenant-scoped, not
    // per-tool, so this checks against every secret the tenant has ever
    // set -- not just the `referenced` subset above, which is this spec's
    // own `secrets` list, a different, narrower thing.
    let env_names: Vec<String> = kind.env_map(&spec).into_keys().collect();
    let mut env_collisions: Vec<String> = Vec::new();
    if !env_names.is_empty() {
        let known_secrets = state.db.list_secret_names(tenant.id).await?;
        for env_name in &env_names {
            if known_secrets.contains(env_name) {
                env_collisions.push(env_name.clone());
            }
        }
    }
    gates.push(publish_gate(
        "env",
        env_collisions.is_empty(),
        (!env_collisions.is_empty()).then(|| {
            format!(
                "env name(s) collide with an existing secret of the same name: {}",
                env_collisions.join(", ")
            )
        }),
        (!env_collisions.is_empty())
            .then(|| "rename the colliding env key(s), or remove the secret first".to_string()),
    ));

    // PRD-grand-loop-billing requirement 3 / PRD-mcphost-tool-versions
    // requirement 2: needed either way now -- `tools_max` below (new tools
    // only) and `versions_max` (every publish, new or republish).
    let plan = state.plans.get(&tenant.plan).ok_or_else(|| {
        AppError::Internal(format!(
            "tenant's plan '{}' is not in the loaded plan catalog",
            tenant.plan
        ))
    })?;

    // gate: network -- PRD-mcphost-sandbox-egress-allowlist requirement 1
    // (AC1/AC2): a `free` tenant publishing `network: "public"` OR
    // `network: "egress"` is refused -- `network_policy::wants_egress`
    // treats both spellings identically. python-only: the other kinds have
    // no `network` concept to gate.
    let mut network_denied = false;
    if kind_name == "python" {
        network_denied = crate::network_policy::wants_egress(spec.get("network").and_then(Value::as_str))
            && plan.name != "pro";
        if network_denied && !dry_run {
            let _ = state.db.record_network_denial("publish_plan", Some(tenant.id)).await;
        }
        gates.push(publish_gate(
            "network",
            !network_denied,
            network_denied
                .then(|| "network: \"public\"/\"egress\" requires the pro plan".to_string()),
            network_denied.then(|| "upgrade to pro, or publish with network: \"none\"".to_string()),
        ));
    }

    // gate: deps -- PRD-mcphost-python-dependency-policy requirement 1/2/3/5:
    // resolve, lock, policy-check and advisory-check a python tool's
    // requirements before anything is stored. Only present when the spec
    // actually declares (or infers) requirements -- a python spec with none
    // has nothing to gate. `spec` gains a `_dependency_lock` field (read
    // back by `kinds::python` at env-build time) and a durable `tool_lock`
    // row is written right after `upsert_tool` below, once `version` is
    // known -- both only for a real (non-`dry_run`) publish that reaches
    // that far.
    let mut pending_lock: Option<(String, i64, String)> = None;
    let mut lock_summary: Option<Value> = None;
    let mut deps_err: Option<AppError> = None;
    if kind_name == "python" {
        let reqs = crate::kinds::python::effective_requirements_for_publish(&spec)?;
        if !reqs.is_empty() {
            let input = crate::deps::classify(&reqs);
            match crate::deps::check_package_policy(&input) {
                Err(e) => {
                    gates.push(publish_gate(
                        "deps",
                        false,
                        Some(e.to_string()),
                        Some("choose a different package, or pin an allowed version".to_string()),
                    ));
                    deps_err = Some(e);
                }
                Ok(()) => {
                    let resolved = crate::deps::resolve(input).await?;
                    let advisory_mode = std::env::var("MCPHOST_ADVISORY_MODE")
                        .unwrap_or_else(|_| "warn".to_string());
                    if advisory_mode == "fail"
                        && let Some(hit) = resolved.advisories.first()
                    {
                        let e = AppError::dependency_advisory(&hit.id, &hit.package, &hit.fixed);
                        gates.push(publish_gate(
                            "deps",
                            false,
                            Some(e.to_string()),
                            Some(format!("upgrade {} to {}, or pin a fixed version", hit.package, hit.fixed)),
                        ));
                        deps_err = Some(e);
                    } else {
                        gates.push(publish_gate("deps", true, None, None));
                        let resolved_unix = now_unix();
                        let advisories_json = crate::deps::advisories_to_json(&resolved.advisories);
                        if let Value::Object(map) = &mut spec {
                            map.insert(
                                "_dependency_lock".to_string(),
                                json!({
                                    "lock_text": resolved.lock_text,
                                    "packages": resolved.packages,
                                    "resolved_unix": resolved_unix,
                                    "advisories": advisories_json,
                                }),
                            );
                        }
                        lock_summary = Some(json!({"packages": resolved.packages, "hashes": true}));
                        pending_lock =
                            Some((resolved.lock_text, resolved_unix, advisories_json.to_string()));
                    }
                }
            }
        }
    }

    let gates_ok = gates.iter().all(|g| g["ok"] == json!(true));
    if dry_run {
        return Ok(json!({"ok": gates_ok, "gates": gates}));
    }
    if !gates_ok {
        // Every existing single-failure test asserts on one of these four
        // errors' own original code/message/data -- `attach_gates` keeps
        // all of that unchanged and only adds `gates`, so a publish that
        // fails more than one at once (AC4) still names its first failure
        // as the top-level error while surfacing every other one too.
        if !missing_secrets.is_empty() {
            return Err(attach_gates(
                AppError::SecretMissing(missing_secrets[0].clone()),
                &gates,
            ));
        }
        if !env_collisions.is_empty() {
            return Err(attach_gates(
                AppError::env_collides_with_secret(&env_collisions[0]),
                &gates,
            ));
        }
        if network_denied {
            return Err(attach_gates(AppError::plan_required("network", "pro"), &gates));
        }
        if let Some(e) = deps_err {
            return Err(attach_gates(e, &gates));
        }
        return Err(AppError::Internal(
            "publish gate reported failure with no recognized cause".to_string(),
        ));
    }

    // A re-publish of an existing name must not count against the limit.
    let already_exists = state.db.get_tool(tenant.id, name.clone()).await?.is_some();
    if !already_exists {
        let count = state.db.count_tools(tenant.id).await?;
        // `MAX_TOOLS_PER_TENANT` becomes the ceiling of any plan's
        // `tools_max` -- an operator hand-editing plans.toml cannot raise a
        // plan past this hard cap.
        let effective_max = plan.tools_max.min(MAX_TOOLS_PER_TENANT);
        if count >= effective_max {
            return Err(crate::billing::quota_exceeded(
                &tenant.plan,
                "tools_max",
                effective_max,
                count,
                None,
            ));
        }
    }

    // PRD-mcphost-tool-versions requirement 2 (AC1/AC3): every publish is a
    // new, immutable version; the oldest beyond `plan.versions_max` is
    // pruned.
    let version = state
        .db
        .upsert_tool(
            tenant.id,
            name.clone(),
            kind_name.clone(),
            spec.clone(),
            plan.versions_max,
        )
        .await?;

    // requirement 1: the durable audit row -- written only once `version`
    // is known, so it lines up with the `tool_versions` row `upsert_tool`
    // just inserted above.
    if let Some((lock_text, resolved_unix, advisories_json)) = pending_lock {
        state
            .db
            .store_tool_lock(
                tenant.id,
                name.clone(),
                version,
                lock_text,
                resolved_unix,
                advisories_json,
            )
            .await?;
    }

    // PRD-mcphost-python-kind-plain-env requirement 4 (AC9): the publish
    // journal row for a spec declaring `env` -- names only, never values,
    // same "journaled (..., never values)" convention `signup`'s handoff
    // token log above already uses for its own sensitive fields.
    if !env_names.is_empty() {
        tracing::info!(
            tenant = %tenant.namespace,
            tool = %name,
            env_names = ?env_names,
            "tool published with plain env entries"
        );
    }

    // PRD-mcphost-code-tools-warm-pool AC3: a republish of an existing name
    // must kill any warm sandbox serving the OLD source before this call
    // returns, not merely let it idle out on its own TTL. Every kind is
    // notified (not just `kind_name`'s own) since a name's kind cannot
    // change across a republish anyway, and the no-op default costs nothing
    // for kinds with no such state.
    if already_exists {
        for k in state.kinds.all() {
            k.on_tool_changed(tenant.id, &name).await;
        }
    }

    // PRD-mcphost-first-call-reliability requirement 4: pre-provision this
    // tool's environment (python-kind: request the build) right after the
    // publish that made it exist, so a call arriving a few seconds later
    // finds it already building/ready instead of discovering "unknown" at
    // call time. Fire only for `kind`, not every registered kind (unlike
    // the eviction above, this isn't a "this name might belong to a
    // different kind now" concern -- it's a fresh provision for the kind
    // that was actually just published) and never awaited past this point
    // by the caller in spirit: `Kind::on_tool_published` impls are expected
    // to spawn/detach their own work (as `PythonKind`'s does via
    // `EnvRegistry::start_build`) rather than block the publish response.
    kind.on_tool_published(tenant.id, &tenant.namespace, &name, &spec)
        .await;

    let mut response = json!({
        "name": format!("{}.{}", tenant.namespace, name),
        "kind": kind_name,
        "version": version,
    });
    // requirement 1 (user story): "host.tool_publish returns lock:
    // {packages: 7, hashes: true}" -- present only for a python tool that
    // actually declared/inferred requirements.
    if let (Some(lock), Value::Object(map)) = (lock_summary, &mut response) {
        map.insert("lock".to_string(), lock);
    }
    Ok(response)
}

/// `host.tool_history(name)` (PRD-mcphost-tool-versions requirement 3,
/// AC1/AC8): every version of `name`, newest-first, `current: true` on the
/// one `tools.current_version` points at. `tool_not_found` for an unknown
/// (or removed -- AC8) name, checked directly against `tools` rather than
/// inferred from an empty version list.
pub async fn tool_history(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let name = arg_str(args, "name")?;
    let tool = state
        .db
        .get_tool(tenant.id, name.clone())
        .await?
        .ok_or_else(|| AppError::ToolNotFound(name.clone()))?;
    let versions = state.db.list_tool_versions(tenant.id, name.clone()).await?;
    let versions: Vec<Value> = versions
        .into_iter()
        .map(|v| {
            json!({
                "version": v.version,
                "created": v.created_at,
                "source_sha256": v.source_sha256,
                "current": v.version == tool.current_version,
            })
        })
        .collect();
    Ok(json!({
        "name": format!("{}.{}", tenant.namespace, name),
        "versions": versions,
    }))
}

/// `host.tool_rollback(name, version)` (requirement 4, AC2/AC4): moves
/// `current_version` to `version` and notifies every `Kind` (same
/// "eviction cost is a no-op for a kind with no such state" convention
/// `tool_remove`/republish already use) so the next unpinned call sees it.
pub async fn tool_rollback(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let name = arg_str(args, "name")?;
    let version = arg_i64(args, "version")?;
    match state.db.rollback_tool_version(tenant.id, name.clone(), version).await? {
        crate::db::RollbackOutcome::NotFound => Err(AppError::ToolNotFound(name)),
        crate::db::RollbackOutcome::OutOfRange { min, max } => {
            Err(AppError::VersionNotFound { requested: version, min, max })
        }
        crate::db::RollbackOutcome::Ok { .. } => {
            for k in state.kinds.all() {
                k.on_tool_changed(tenant.id, &name).await;
            }
            Ok(json!({
                "name": format!("{}.{}", tenant.namespace, name),
                "version": version,
            }))
        }
    }
}

/// `host.tool_diff(name, from, to)` (requirement 8, P2/AC9): a unified diff
/// between two versions' own stored spec (pretty-printed JSON, since a spec
/// isn't always just a `source` string -- `echo`/`http` specs have no such
/// field at all).
pub async fn tool_diff(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let name = arg_str(args, "name")?;
    let from = arg_i64(args, "from")?;
    let to = arg_i64(args, "to")?;
    state
        .db
        .get_tool(tenant.id, name.clone())
        .await?
        .ok_or_else(|| AppError::ToolNotFound(name.clone()))?;
    let range = state.db.tool_version_range(tenant.id, name.clone()).await?;
    let (min, max) = range.unwrap_or((1, 1));
    let from_row = state
        .db
        .get_tool_version(tenant.id, name.clone(), from)
        .await?
        .ok_or(AppError::VersionNotFound { requested: from, min, max })?;
    let to_row = state
        .db
        .get_tool_version(tenant.id, name.clone(), to)
        .await?
        .ok_or(AppError::VersionNotFound { requested: to, min, max })?;
    let from_text = serde_json::to_string_pretty(&from_row.spec).unwrap_or_default();
    let to_text = serde_json::to_string_pretty(&to_row.spec).unwrap_or_default();
    let diff = crate::difftext::unified_diff(
        &from_text,
        &to_text,
        &format!("v{from}"),
        &format!("v{to}"),
    );
    Ok(json!({
        "name": format!("{}.{}", tenant.namespace, name),
        "from": from,
        "to": to,
        "diff": diff,
    }))
}

pub async fn tool_list(state: &AppState, tenant: &Tenant) -> Result<Value, AppError> {
    let rows = state.db.list_tools(tenant.id).await?;
    // PRD-mcphost-python-dependency-policy requirement 6 (AC8): read fresh
    // from `tool_lock`, not the tool's immutable stored `spec` -- a daily
    // re-audit updates this count in place, with no republish, so this
    // must see that update on the very next `host.tool_list` (see
    // `deps::reaudit_once`).
    let advisory_counts = state
        .db
        .current_tool_lock_advisory_counts_for_tenant(tenant.id)
        .await?;
    let tools: Vec<Value> = rows
        .into_iter()
        .map(|row| {
            // PRD-mcphost-python-kind-plain-env requirement 5 (AC7): this
            // is the "descriptor surface that returns a tool's parsed
            // spec" for a caller wanting to know a tool's env map --
            // `host.tool_list` already loads each row's full `spec`, so
            // this is a plain, additive read of it via the kind's own
            // `env_map`, empty (never omitted) for a kind or spec with
            // none.
            let env = state
                .kinds
                .get(&row.kind)
                .map(|kind| kind.env_map(&row.spec))
                .unwrap_or_default();
            // requirement 6 (AC8): 0 for a tool with no stored lock (not
            // `python`, or `python` with no requirements) -- never omitted.
            let advisories = advisory_counts.get(&row.name).copied().unwrap_or(0);
            let mut entry = json!({
                "name": format!("{}.{}", tenant.namespace, row.name),
                "kind": row.kind,
                "created_at": row.created_at,
                // PRD-mcphost-sharing requirement 1/AC9: an owner sees its
                // own tool's share state directly here -- `unshared_by` is
                // `"admin"` only when `admin.tool_unshare` (not the owner's
                // own `host.tool_unshare`) most recently forced it private.
                "visibility": row.visibility,
                "share_description": row.share_description,
                "unshared_by": row.unshared_by,
                "env": env,
                "advisories": advisories,
            });
            // PRD-mcphost-first-publish-real-kind requirement 5 (AC5):
            // `stub: true` on an echo-kind tool, absent (not `false`) for
            // every other kind -- same derivation as `tools/list`'s own
            // `_meta.stub`.
            if row.kind == "echo"
                && let Value::Object(map) = &mut entry
            {
                map.insert("stub".to_string(), json!(true));
            }
            entry
        })
        .collect();
    Ok(json!({ "tools": tools }))
}

pub async fn tool_remove(
    state: &AppState,
    tenant: &Tenant,
    args: &Value,
) -> Result<Value, AppError> {
    let name = arg_str(args, "name")?;
    let removed = state.db.remove_tool(tenant.id, name.clone()).await?;
    if !removed {
        return Err(AppError::ToolNotFound(name));
    }
    // PRD-mcphost-code-tools-warm-pool AC3: same "killed before the
    // operation returns" guarantee as a republish, for an outright removal.
    for k in state.kinds.all() {
        k.on_tool_changed(tenant.id, &name).await;
    }
    // PRD-mcphost-schedules P0 requirement 1 / AC7: a tool remove disables
    // (never deletes -- `host.trigger.list` still shows the history) every
    // trigger set on it, reporting how many.
    let triggers_disabled = state.db.disable_triggers_for_tool(tenant.id, name.clone()).await?;
    Ok(json!({ "removed": name, "triggers_disabled": triggers_disabled }))
}

pub async fn tool_logs(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let name = arg_str(args, "name")?;
    // PRD-mcphost-tool-test AC12: `host.spec_test`'s invocations are logged
    // under a per-kind synthetic bucket name (`__spec_test__.<kind>`),
    // never a `tools` row -- accept that bucket here too (for a currently
    // registered kind) so a tenant can read test-invocation logs back
    // through this same RPC rather than needing a second one. A tenant's
    // own published tool still satisfies the lookup on its own terms
    // either way, so this is purely additive.
    let is_test_bucket = name
        .strip_prefix("__spec_test__.")
        .is_some_and(|kind| state.kinds.get(kind).is_some());
    if !is_test_bucket && state.db.get_tool(tenant.id, name.clone()).await?.is_none() {
        return Err(AppError::ToolNotFound(name));
    }
    let limit = args
        .get("limit")
        .and_then(Value::as_i64)
        .unwrap_or(20)
        .clamp(1, 1000);
    let lines = state.db.tail_logs(tenant.id, name.clone(), limit).await?;
    Ok(json!({ "name": name, "lines": lines }))
}

/// PRD-mcphost-shared-tool-caller-usage requirement 2 (AC1/AC2/AC4/AC5/AC8):
/// the `by` breakdown path. `window` defaults to `"1d"` here (rather than
/// the legacy per-tenant `usage`'s `"24h"`) since every acceptance
/// criterion names day-granular windows and this is a wholly new response
/// shape with no pre-existing default to preserve.
async fn usage_breakdown(state: &AppState, tenant: &Tenant, args: &Value, by: String) -> Result<Value, AppError> {
    if !matches!(by.as_str(), "tool" | "caller" | "end_user") {
        return Err(AppError::InvalidArgs(format!(
            "by must be 'tool', 'caller', or 'end_user', got '{by}'"
        )));
    }
    let tool = arg_str_opt(args, "tool");
    let window = arg_str_opt(args, "window").unwrap_or_else(|| "1d".to_string());
    let secs = crate::state::parse_window_secs(&window);
    let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(1000).clamp(1, 1000);
    let cursor = arg_str_opt(args, "cursor");

    // Requirement 2: "by: 'caller' is valid only for shared tools" (AC4) --
    // checked here, before any aggregation runs, so a validation mistake
    // never depends on how much data happens to already exist.
    if by == "caller" {
        let name = tool.clone().ok_or_else(|| {
            AppError::InvalidArgs("by: 'caller' requires 'tool'".to_string())
        })?;
        let row = state
            .db
            .get_tool(tenant.id, name.clone())
            .await?
            .ok_or_else(|| AppError::ToolNotFound(name.clone()))?;
        if row.visibility == "private" {
            return Err(AppError::InvalidArgs(format!(
                "by: 'caller' requires tool '{name}' to be shared (visibility is 'private'); \
                 share it first with host.tool_share"
            )));
        }
    }

    let result = state
        .db
        .usage_breakdown(tenant.id, tool, by.clone(), secs, limit, cursor)
        .await?;
    Ok(json!({
        "window": window,
        "by": by,
        "rows": result.rows.iter().map(|r| json!({
            "key": r.key,
            "calls": r.calls,
            "errors": r.errors,
            "p95_ms": r.p95_ms,
            "bytes_out": r.bytes_out,
        })).collect::<Vec<_>>(),
        "cursor": result.next_cursor,
    }))
}

pub async fn usage(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    // AC6: `by` omitted keeps the pre-PRD per-tenant shape byte for byte --
    // every field below this point is unchanged from before this PRD.
    if let Some(by) = arg_str_opt(args, "by") {
        return usage_breakdown(state, tenant, args, by).await;
    }
    let window = arg_str_opt(args, "window").unwrap_or_else(|| "24h".to_string());
    let secs = crate::state::parse_window_secs(&window);
    let stats = state.db.usage(tenant.id, secs).await?;
    // PRD-mcphost-tenant-state requirement 5 / AC7: cumulative, not
    // windowed -- `state_bytes_used` sums the tenant's whole store right
    // now, the same total `admin.tenants`' own `state_bytes` (requirement
    // 8, P1) will report.
    let state_bytes = state.db.state_bytes_used(tenant.id).await?;
    // PRD-mcphost-run-result-overflow-to-state requirement 5 / AC6: the
    // slice of `state_bytes` above that's run-result parts, reported
    // separately so a `host.runs.purge` is visible here without also
    // moving the rest of a tenant's own state.
    let run_results_bytes = state.db.run_results_bytes(tenant.id).await?;
    // PRD-mcphost-sharing requirement 3 (AC5): `calls_by_others` (owner
    // side, keyed by caller namespace) and `calls_to_shared` (caller
    // side) -- windowed the same as every other figure in this response.
    let calls_by_others = state.db.calls_by_others(tenant.id, secs).await?;
    let calls_by_others: serde_json::Map<String, Value> = calls_by_others
        .into_iter()
        .map(|(ns, n)| (ns, json!(n)))
        .collect();
    let calls_to_shared = state.db.calls_to_shared(tenant.id, secs).await?;
    // PRD-mcphost-runs-and-jobs P0 requirement 7: `host.usage` gains
    // `jobs: {done, error, timeout, seconds}` -- same window as everything
    // else in this response.
    let jobs = state.db.jobs_usage(tenant.id, secs).await?;
    // PRD-mcphost-schedules P0 requirement 5: `host.usage` counts scheduled
    // runs the same window every other figure here uses.
    let scheduled = state.db.scheduled_usage(tenant.id, secs).await?;
    // PRD-mcphost-data-retention P1 requirement 5 (AC8): the retention
    // windows every tenant's data is subject to -- host-wide, not
    // per-tenant, since retention is a host policy (requirement 1).
    let retention_windows = state.db.retention_windows().await?;
    let retention_days: serde_json::Map<String, Value> = retention_windows
        .into_iter()
        .map(|(table, days)| (table, json!(days)))
        .collect();
    Ok(json!({
        "window": window,
        "calls": stats.calls,
        "errors": stats.errors,
        "p50_ms": stats.p50_ms,
        "p95_ms": stats.p95_ms,
        "state_bytes": state_bytes,
        "run_results_bytes": run_results_bytes,
        // PRD-mcphost-call-limits-honest AC8: `Db::usage` already counts
        // this from `error_class = "capacity"`; this handler just wasn't
        // forwarding it into the response envelope.
        "capacity_refusals": stats.capacity_refusals,
        "calls_by_others": calls_by_others,
        "calls_to_shared": calls_to_shared,
        "jobs": {
            "done": jobs.done,
            "error": jobs.error,
            "timeout": jobs.timeout,
            "cancelled": jobs.cancelled,
            "seconds": jobs.seconds,
        },
        // PRD-mcphost-schedules P0 requirement 5.
        "scheduled": {
            "done": scheduled.done,
            "error": scheduled.error,
            "timeout": scheduled.timeout,
            "cancelled": scheduled.cancelled,
            "skipped": scheduled.skipped,
            "seconds": scheduled.seconds,
        },
        "retention_days": retention_days,
    }))
}

pub async fn secret_set(
    state: &AppState,
    tenant: &Tenant,
    args: &Value,
) -> Result<Value, AppError> {
    let name = arg_str(args, "name")?;
    let value = arg_str(args, "value")?;

    // PRD-mcphost-end-user-identity requirement 2: the reserved secret name
    // `assertion_secret_rotate` alone writes to -- a tenant setting this
    // name directly would let it choose (and therefore know) its own
    // assertion secret, defeating "the secret value appears in neither
    // logs nor host.secret_list output" (AC11: only a rotation the host
    // itself generated is ever trustworthy as unguessable).
    if name == crate::enduser::ASSERTION_SECRET_NAME {
        return Err(AppError::InvalidArgs(format!(
            "name: '{name}' is reserved; use host.enduser.assertion_secret_rotate instead"
        )));
    }

    // PRD-mcphost-python-kind-plain-env requirement 3 (AC4), the symmetric
    // direction of `tool_publish`'s own env/secret collision check above: a
    // new secret name must not collide with any of this tenant's
    // already-published tools' `env` entries. Secrets are tenant-scoped
    // (not per-tool), so every one of this tenant's tools needs checking,
    // not just whichever tool happens to reference this secret name.
    let tools = state.db.list_tools(tenant.id).await?;
    for row in &tools {
        if let Some(kind) = state.kinds.get(&row.kind)
            && kind.env_map(&row.spec).contains_key(&name)
        {
            return Err(AppError::secret_collides_with_env(&name));
        }
    }

    // PRD-grand-loop-billing requirement 3: `secrets_max` enforcement,
    // same shape as `tool_publish`'s `tools_max` check above. A re-set of
    // an existing secret name must not count against the limit, same
    // rationale as a tool republish.
    let already_exists = state.db.list_secret_names(tenant.id).await?.contains(&name);
    if !already_exists {
        let plan = state.plans.get(&tenant.plan).ok_or_else(|| {
            AppError::Internal(format!(
                "tenant's plan '{}' is not in the loaded plan catalog",
                tenant.plan
            ))
        })?;
        let count = state.db.count_secrets(tenant.id).await?;
        if count >= plan.secrets_max {
            return Err(crate::billing::quota_exceeded(
                &tenant.plan,
                "secrets_max",
                plan.secrets_max,
                count,
                None,
            ));
        }
    }

    let (ct, nonce) = state.secrets.encrypt(&value)?;
    state
        .db
        .upsert_secret(tenant.id, name.clone(), ct, nonce)
        .await?;
    Ok(json!({ "name": name, "set": true }))
}

pub async fn secret_list(state: &AppState, tenant: &Tenant) -> Result<Value, AppError> {
    let names = state.db.list_secret_names(tenant.id).await?;
    Ok(json!({ "names": names }))
}

/// AC19 / requirement 15: publish this tenant's `server.json` to the
/// configured registry API and serve it locally at
/// `/.well-known/mcp/<namespace>/server.json`. Refuses with a distinct
/// error when the feature flag is off (`AppError::RegistryDisabled`) or
/// this tenant's domain namespace has not been admin-verified
/// (`AppError::NamespaceUnverified`); a non-2xx from the registry API
/// propagates as `AppError::RegistryRejected`, which does NOT leave a
/// stale document being served (the DB write only happens after the
/// registry itself accepts it).
pub async fn registry_publish(
    state: &AppState,
    tenant: &Tenant,
    _args: &Value,
) -> Result<Value, AppError> {
    let registry = state.registry.as_ref().ok_or(AppError::RegistryDisabled)?;
    if !tenant.namespace_verified {
        return Err(AppError::NamespaceUnverified);
    }
    let domain_namespace = tenant
        .registry_namespace
        .clone()
        .ok_or(AppError::NamespaceUnverified)?;

    let endpoint_url = format!("{}/mcp", state.public_url.trim_end_matches('/'));
    let document =
        crate::registry::build_server_json(&domain_namespace, &tenant.display_name, &endpoint_url);

    let publish_url = format!("{}/v0/publish", registry.base_url.trim_end_matches('/'));
    let resp = state
        .http_client
        .post(&publish_url)
        .json(&document)
        .send()
        .await
        .map_err(|e| AppError::RegistryRejected(format!("request to registry API failed: {e}")))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(AppError::RegistryRejected(format!(
            "registry API returned HTTP {status}"
        )));
    }

    state
        .db
        .upsert_registry_document(tenant.id, tenant.namespace.clone(), document.clone())
        .await?;

    Ok(json!({
        "namespace": tenant.namespace,
        "domain_namespace": domain_namespace,
        "well_known_url": format!(
            "{}/.well-known/mcp/{}/server.json",
            state.public_url.trim_end_matches('/'),
            tenant.namespace,
        ),
        "server_json": document,
        "registry_status": status.as_u16(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_header_validation() {
        assert_eq!(validate_synthetic_header(None), None);
        assert_eq!(validate_synthetic_header(Some("")), None);
        assert_eq!(
            validate_synthetic_header(Some("synthorg:run-a")),
            Some("synthorg:run-a".to_string())
        );
        assert_eq!(validate_synthetic_header(Some("operator")), Some("operator".to_string()));
        // Invalid input degrades to null (and logs a warning -- see the
        // integration test AC3 for the warning itself), never an error.
        assert_eq!(validate_synthetic_header(Some("Bad Label!")), None);
        assert_eq!(validate_synthetic_header(Some(" ")), None);
    }
}
