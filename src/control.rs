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

/// PRD-mcphost-handoff-token requirement 1: `signup`'s `handoff` argument.
/// Absent, `null`, or any non-`bool` value all read as `false` (the
/// existing raw-key behavior, requirement 5 / AC5 -- an old client that has
/// never heard of this argument must see byte-identical output), so only an
/// explicit `true` opts into handoff mode.
fn arg_bool(args: &Value, name: &str) -> bool {
    args.get(name).and_then(Value::as_bool).unwrap_or(false)
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
    let display_name = arg_str(args, "name")?;

    // Requirement 1: `source_class` first (loopback IP or the harness
    // marker header; known-fleet display name or synthorg client name;
    // else external), then `tenants.synthetic` from it -- the explicit
    // stamp when the header carried a valid one, else `harness:unstamped`
    // for loopback/fleet, else `None` for a real (`external`) tenant.
    let explicit_label = validate_synthetic_header(attribution.synthetic_header);
    let harness_marker_present = attribution.synthetic_header.is_some();
    let class = crate::state::classify_source_class(
        source_ip,
        harness_marker_present,
        &display_name,
        attribution.client_name,
    );
    let synthetic = if class.is_synthetic() {
        Some(explicit_label.unwrap_or_else(|| "harness:unstamped".to_string()))
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
        .create_tenant_attributed(
            display_name,
            namespace.clone(),
            key_hash,
            synthetic,
            Some(class.as_str().to_string()),
            attribution.client_name.map(str::to_string),
            attribution.client_version.map(str::to_string),
            origin.to_string(),
            origin_detail,
        )
        .await?;

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
        return Ok(json!({
            "tenant": tenant.namespace,
            "tenant_id": tenant.namespace,
            "handoff_token": token,
            "expires_in": HANDOFF_TOKEN_TTL_SECS,
            "usage": "Call host.redeem with this handoff_token (no Authorization header or \
                tenant_key needed) to receive your tenant key exactly once. The token is \
                single-use and expires in expires_in seconds -- a transcript that captured \
                this response is worthless to anyone who reads it after redemption.",
            "next": "host.redeem",
        }));
    }

    Ok(json!({
        "tenant": tenant.namespace,
        "key": key,
        "namespace": namespace,
        "endpoint": format!("{}/mcp", state.public_url.trim_end_matches('/')),
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
    }))
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

    let kind_name = arg_str(args, "kind")?;
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
        try_before_call.push(json!({
            "case": "a published python tool, for stdout, stderr and exit code",
            "call": "host.tool_run",
            "arguments": {"name": tool_name, "args": python_example.call_args},
        }));
    }

    Ok(json!({
        "authenticated": true,
        "namespace": tenant.namespace,
        "kind": kind_name,
        "try_before_call": try_before_call,
        "steps": [
            {
                "call": "host.tool_publish",
                "arguments": {"name": tool_name, "kind": kind_name, "spec": example.spec},
                "note": "Publish the tool. A rejection names the field, what was expected, \
                    and a corrected example -- fix it and resubmit.",
            },
            {
                "call": "host.tool_test",
                "arguments": {"name": tool_name, "args": example.call_args},
                "note": "Dry-run it: the real call, but it counts toward neither \
                    host.usage nor host.tool_logs, so it's safe to repeat while iterating.",
            },
            {
                "call": qualified_name,
                "arguments": example.call_args.clone(),
                "alternative_call": "host.tool_call",
                "alternative_arguments": {"name": tool_name, "args": example.call_args},
                "note": "The real call, either by its namespaced name directly or via \
                    host.tool_call by local name -- identical for metering and logs.",
            },
            {
                "call": "host.state.set",
                "arguments": {"key": "example", "value": {"n": 1}},
                "note": "Optional: remember something between calls. host.state.get(key) \
                    reads it back; a python tool's own code can read/write the same store. \
                    See host.state.table_create for typed tables.",
            },
        ],
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

pub fn whoami(tenant: &Tenant) -> Value {
    // PRD-mcphost-handoff-token P1 requirement 7 / AC8: age is measured
    // from the last rotation when there's been one, else from the
    // tenant's own creation -- a never-rotated key is exactly as old as
    // the tenant. `created_unix` is only `None` for a row this crate never
    // wrote (should not happen post-migration-0010), in which case age is
    // unknowable rather than a misleading guess.
    let key_since = tenant.key_rotated_unix.or(tenant.created_unix);
    let key_age_s = key_since.map(|since| (now_unix() - since).max(0));
    json!({
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
    })
}

pub async fn tool_publish(
    state: &AppState,
    tenant: &Tenant,
    args: &Value,
) -> Result<Value, AppError> {
    let name = arg_str(args, "name")?;
    let kind_name = arg_str(args, "kind")?;
    let spec = args.get("spec").cloned().unwrap_or(Value::Null);

    validate_tool_name(&name)?;

    let spec_bytes = serde_json::to_vec(&spec)
        .map_err(|e| AppError::Internal(format!("spec serialize: {e}")))?
        .len();
    if spec_bytes > MAX_SPEC_BYTES {
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
    if let Some(status) = kind.sandbox_status()
        && !status.ready
    {
        return Err(AppError::sandbox_unavailable(&status));
    }

    // Requirement 3 / AC2: every simultaneously-failing field is reported
    // at once, not just the first -- see `AppError::from_kind_violations`.
    if let Some(err) = AppError::from_kind_violations(kind.validate_all(&spec)) {
        return Err(err);
    }
    kind.validate_async(&spec).await?;

    // Requirement 3 / AC3: every `secret.<name>` the spec references must
    // already exist for this tenant, checked before the tool is ever
    // stored. Kinds with no secret-templating concept (`echo`) return no
    // references here, so this is a no-op for them.
    let referenced = kind.referenced_secrets(&spec);
    if !referenced.is_empty() {
        let known = state.db.list_secret_names(tenant.id).await?;
        for secret_name in referenced {
            if !known.contains(&secret_name) {
                return Err(AppError::SecretMissing(secret_name));
            }
        }
    }

    // A re-publish of an existing name must not count against the limit.
    let already_exists = state.db.get_tool(tenant.id, name.clone()).await?.is_some();
    if !already_exists {
        let count = state.db.count_tools(tenant.id).await?;
        // PRD-grand-loop-billing requirement 3: `MAX_TOOLS_PER_TENANT`
        // becomes the ceiling of any plan's `tools_max` -- an operator
        // hand-editing plans.toml cannot raise a plan past this hard cap.
        let plan = state.plans.get(&tenant.plan).ok_or_else(|| {
            AppError::Internal(format!(
                "tenant's plan '{}' is not in the loaded plan catalog",
                tenant.plan
            ))
        })?;
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

    state
        .db
        .upsert_tool(tenant.id, name.clone(), kind_name.clone(), spec.clone())
        .await?;

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

    Ok(json!({
        "name": format!("{}.{}", tenant.namespace, name),
        "kind": kind_name,
    }))
}

pub async fn tool_list(state: &AppState, tenant: &Tenant) -> Result<Value, AppError> {
    let rows = state.db.list_tools(tenant.id).await?;
    let tools: Vec<Value> = rows
        .into_iter()
        .map(|row| {
            json!({
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
            })
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

pub async fn usage(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let window = arg_str_opt(args, "window").unwrap_or_else(|| "24h".to_string());
    let secs = crate::state::parse_window_secs(&window);
    let stats = state.db.usage(tenant.id, secs).await?;
    // PRD-mcphost-tenant-state requirement 5 / AC7: cumulative, not
    // windowed -- `state_bytes_used` sums the tenant's whole store right
    // now, the same total `admin.tenants`' own `state_bytes` (requirement
    // 8, P1) will report.
    let state_bytes = state.db.state_bytes_used(tenant.id).await?;
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
    Ok(json!({
        "window": window,
        "calls": stats.calls,
        "errors": stats.errors,
        "p50_ms": stats.p50_ms,
        "p95_ms": stats.p95_ms,
        "state_bytes": state_bytes,
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
    }))
}

pub async fn secret_set(
    state: &AppState,
    tenant: &Tenant,
    args: &Value,
) -> Result<Value, AppError> {
    let name = arg_str(args, "name")?;
    let value = arg_str(args, "value")?;

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
