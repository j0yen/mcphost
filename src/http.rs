//! The axum app: `GET /healthz` (unauthenticated) and `POST /mcp` (the
//! `rmcp` stateless streamable-HTTP service, advertising whatever
//! protocol version this build of `rmcp` actually negotiates -- see
//! [`advertised_protocol_version`].

use std::collections::HashMap;
use std::net::SocketAddr;
use std::os::fd::FromRawFd;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use axum::body::{Body, Bytes};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, Request, StatusCode, header::CACHE_CONTROL};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rmcp::model::ProtocolVersion;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use serde_json::{Value, json};

use crate::handler::McpHostHandler;
use crate::state::{AppState, MAX_REQUEST_BODY_BYTES};

/// The `MCP-Protocol-Version` value this server advertises, derived from
/// `rmcp::model::ProtocolVersion::LATEST` rather than a string literal
/// (PRD-mcphost-protocol-compat requirement 6): the version this
/// middleware stamps onto responses must always be the version `rmcp`
/// actually negotiates, so an `rmcp` upgrade that moves `LATEST` either
/// moves this advertisement with it or fails requirement 7's test. The
/// `HeaderValue` is computed once and cached, since `HeaderValue::from_str`
/// is fallible and `HeaderValue::from_static` needs a `&'static str` we
/// don't have at compile time.
fn advertised_protocol_version() -> Option<&'static HeaderValue> {
    static VALUE: OnceLock<Option<HeaderValue>> = OnceLock::new();
    VALUE
        .get_or_init(|| HeaderValue::from_str(ProtocolVersion::LATEST.as_str()).ok())
        .as_ref()
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// PRD-mcphost-healthz-minimal requirements 2-4: only the configured
/// `MCPHOST_ADMIN_KEY` bearer unlocks the full diagnostics document. A
/// tenant key hashes against `state.db`, not `state.admin_key`, so it is
/// deliberately never checked here (requirement 5/AC5) -- widening this to
/// "any known key" would let a paying tenant read every tenant's counts.
/// Comparison is constant-time so a wrong key takes the same time as no key
/// (requirement 4).
fn is_admin_request(state: &AppState, headers: &HeaderMap) -> bool {
    let Some(admin_key) = state.admin_key.as_deref() else {
        return false;
    };
    let Some(bearer) = crate::auth::extract_bearer(headers) else {
        return false;
    };
    constant_time_eq(bearer.as_bytes(), admin_key.as_bytes())
}

/// PRD-mcphost-healthz-minimal requirement 1: the anonymous body is
/// `{"ok": true}` (200) or `{"ok": false}` (503) and nothing else --
/// `paying_tenants`/`tenants_total`/`tools_total`/`billing_mode`/
/// `sandbox_*`/`version` are business metrics and reconnaissance-grade
/// facts that used to leak to any unauthenticated caller. The full
/// document (unchanged shape from before this PRD) now requires the admin
/// bearer; a wrong or absent key both fall through to the same anonymous
/// body (requirement 3/AC3) since [`is_admin_request`] doesn't distinguish
/// "no header" from "wrong header".
async fn healthz(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    let mut response = healthz_response(&state, &headers).await;
    // PRD-mcphost-checkcompat-port-race requirement 5: the token header is
    // added only when `$MCPHOST_COMPAT_TOKEN` was set in this process's own
    // env at startup -- production `serve` never sets it, so tenants never
    // see it.
    if let Some(token) = state.compat_token.as_deref()
        && let Ok(value) = HeaderValue::from_str(token)
    {
        response.headers_mut().insert("X-Mcphost-Compat-Token", value);
    }
    response
}

async fn healthz_response(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let db_ok = state.db.is_writable().await;
    // PRD-mcphost-data-retention requirement 4 (AC6): the disk-floor guard
    // is as much a liveness signal as `db_ok` -- a box below the floor
    // refuses every write the same way an unwritable database does, so it
    // folds into the anonymous `ok` the same way (AC14 precedent).
    let disk_ok = state.disk_guard.is_ok(state.db.data_dir());

    if !is_admin_request(state, headers) {
        return if db_ok && disk_ok {
            (StatusCode::OK, Json(json!({"ok": true}))).into_response()
        } else {
            (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"ok": false}))).into_response()
        };
    }

    let (tools_total, tenants_total) = state.db.counts().await.unwrap_or((0, 0));
    // PRD-mcphost-tenant-delete requirement 5 / AC9: `tenants_probe` is
    // additive -- `tenants_total` (read by `mcphost-deploy probe` and the
    // measure job today) is unchanged, so `tenants_total - tenants_probe`
    // is the real-tenant count.
    let tenants_probe = state.db.probe_tenant_count().await.unwrap_or(0);
    // PRD-mcphost-tenant-attribution requirement 3 / AC1/AC3/AC4:
    // `tenants_real` now counts only `source_class = 'external'` --
    // `synthetic IS NULL` (PRD-mcphost-synthetic-flag's original
    // definition) stopped being a safe proxy for "real" the moment
    // migration 0010 started labeling every loopback/fleet signup
    // `harness:unstamped` too, which is exactly the correction this PRD's
    // TL;DR describes ("95 real tenants; there are zero"). `tenants_synthetic`
    // is always present (0 when none), same as before.
    let tenants_real = state.db.count_external_tenants().await.unwrap_or(0);
    let tenants_synthetic = tenants_total - tenants_real;
    // PRD-mcphost-provenance-audit requirement 3: the unqualified
    // `tenants_real`/`tenants_synthetic` fields above are replaced by a
    // nested `{external, synthetic}` object -- no field named "real" may
    // aggregate both provenance classes under a name that implies it's
    // pure. `signups`/`calls` get the same shape, from the write-time
    // `origin` column migration 0012 added (independent of the
    // `source_class`-keyed computation above, which
    // `count_external_tenants` still serves for its own narrower purpose).
    let (signups_external, signups_synthetic) =
        state.db.count_signup_events_by_origin().await.unwrap_or((0, 0));
    let (calls_external, calls_synthetic) =
        state.db.count_calls_by_origin().await.unwrap_or((0, 0));
    let mut body = json!({
        "version": env!("CARGO_PKG_VERSION"),
        "db_ok": db_ok,
        "disk_ok": disk_ok,
        "tools_total": tools_total,
        "tenants_total": tenants_total,
        "tenants_probe": tenants_probe,
        "tenants": {"external": tenants_real, "synthetic": tenants_synthetic},
        "signups": {"external": signups_external, "synthetic": signups_synthetic},
        "calls": {"external": calls_external, "synthetic": calls_synthetic},
        "sandbox_mechanism": state.sandbox_mechanism,
        "wasm_runtime_version": state.wasm_runtime_version,
    });
    // PRD-mcphost-tenant-attribution requirement 3: `tenants_by_source_class`
    // (every class present, most common first) and `tenants_by_client`
    // (top 10 `clientInfo.name` values by tenant count) -- both additive,
    // both empty arrays (not absent) on a box with nothing classified yet.
    if let Some(obj) = body.as_object_mut() {
        let by_class = state
            .db
            .count_tenants_by_source_class()
            .await
            .unwrap_or_default();
        obj.insert(
            "tenants_by_source_class".to_string(),
            json!(
                by_class
                    .into_iter()
                    .map(|(class, count)| json!({"source_class": class, "count": count}))
                    .collect::<Vec<_>>()
            ),
        );
        let by_client = state
            .db
            .count_tenants_by_client(10)
            .await
            .unwrap_or_default();
        obj.insert(
            "tenants_by_client".to_string(),
            json!(
                by_client
                    .into_iter()
                    .map(|(name, count)| json!({"client_name": name, "count": count}))
                    .collect::<Vec<_>>()
            ),
        );
    }
    // PRD-mcphost-signup-kill-switch-and-source requirement 2 / AC6: a
    // caller-claimed-channel breakdown of external signups, all-time and
    // (requirement 2's other named window) the last 24h -- both additive,
    // both `{}` (not absent) on a box with no sourced signups yet.
    // Requirement 5 / AC7: `signups_enabled` mirrors whether the pause file
    // exists right now (same fresh-stat-per-call contract `signup` itself
    // uses); `signup_pause_message` is present only while paused, matching
    // requirement 5's own "and `signup_pause_message` when paused" wording
    // rather than a present-and-null field on every unpaused host.
    if let Some(obj) = body.as_object_mut() {
        let by_source = state
            .db
            .count_external_signups_by_source(None)
            .await
            .unwrap_or_default();
        let by_source_24h = state
            .db
            .count_external_signups_by_source(Some(crate::state::now_unix() - 86_400))
            .await
            .unwrap_or_default();
        obj.insert(
            "signups_by_source".to_string(),
            json!({"external": by_source.into_iter().collect::<std::collections::BTreeMap<_, _>>()}),
        );
        obj.insert(
            "signups_by_source_24h".to_string(),
            json!({"external": by_source_24h.into_iter().collect::<std::collections::BTreeMap<_, _>>()}),
        );
        let pause = state.signup_pause.status();
        obj.insert("signups_enabled".to_string(), json!(pause.is_none()));
        if let Some(pause) = pause {
            obj.insert("signup_pause_message".to_string(), json!(pause.message));
        }
    }
    // PRD-mcphost-sandbox-ready requirement 2: additive fields, populated
    // from an actual sandboxed-process probe rather than the `on_path`
    // check `sandbox_mechanism` above has always been. `None` (absent
    // fields) when no registered kind has a self-test to report -- an
    // `echo`/`http`-only deployment is unaffected (migration note:
    // "existing clients that ignore unknown fields are unaffected").
    if let Some(status) = state.kinds.all().find_map(|k| k.sandbox_status())
        && let Some(obj) = body.as_object_mut()
    {
        obj.insert("sandbox_ready".to_string(), json!(status.ready));
        obj.insert("sandbox_detail".to_string(), json!(status.detail));
        obj.insert("sandbox_checked_at".to_string(), json!(status.checked_at));
    }
    // PRD-mcphost-client-ip-behind-proxy requirement 5 / AC6: additive,
    // always present (0 on a box with no signups in the last 24h) --
    // `count_distinct_source_ips_since` reads the same `signup_events`
    // table the rate limiter and `resolve_source_ip` now populate with the
    // Caddy-forwarded address instead of the shared loopback one.
    if let Some(obj) = body.as_object_mut() {
        let distinct = state
            .db
            .count_distinct_source_ips_since(crate::state::now_unix() - 86_400)
            .await
            .unwrap_or(0);
        obj.insert("distinct_source_ips_24h".to_string(), json!(distinct));
    }
    // PRD-grand-loop-billing AC4/AC11: `off` with no Stripe key, else
    // `test`/`live` from the key prefix, plus a live count of tenants on
    // any paid plan -- additive fields, ignored by an old client the same
    // way the sandbox fields above are.
    if let Some(obj) = body.as_object_mut() {
        obj.insert(
            "billing_mode".to_string(),
            json!(state.billing_config.billing_mode()),
        );
        obj.insert(
            "paying_tenants".to_string(),
            json!(state.db.count_paying_tenants().await.unwrap_or(0)),
        );
    }
    // PRD-mcphost-agent-mesh-ops requirement 6 / AC8: additive, always
    // present (0s on a box with no mesh traffic yet) -- the operator's
    // first fact about the mesh beyond the tenant/tool counts above.
    if let Some(obj) = body.as_object_mut() {
        let (messages_24h, posts_24h, frozen_tenants) =
            state.db.mesh_healthz_counts().await.unwrap_or((0, 0, 0));
        obj.insert(
            "mesh".to_string(),
            json!({
                "messages_24h": messages_24h,
                "posts_24h": posts_24h,
                "frozen_tenants": frozen_tenants,
            }),
        );
    }
    // PRD-mcphost-synthetic-flag P1 requirement 6 / AC10: `paying_tenants_real`
    // appears only once a labeled tenant has actually gone paid -- absent,
    // not present-and-equal, on every host where it can never have
    // differed from `paying_tenants` yet (same absent-until-relevant
    // pattern as `meter_lag` below).
    if let Ok((paying_real, paying_synthetic)) = state.db.paying_tenant_synthetic_split().await
        && paying_synthetic > 0
        && let Some(obj) = body.as_object_mut()
    {
        obj.insert("paying_tenants_real".to_string(), json!(paying_real));
    }
    // PRD-mcphost-metered-overage AC8: `meter_lag` only appears when
    // metering is configured (`metered_price_id` set) -- unconfigured, the
    // key is absent entirely, not present-and-zero, so a caller can
    // distinguish "no overage billing on this host" from "0 calls behind."
    if state.billing_config.metered_price_id.is_some()
        && let Some(obj) = body.as_object_mut()
    {
        let (last_call_id, _) = state.db.get_meter_state().await.unwrap_or((0, None));
        let lag = state.db.meter_lag(last_call_id).await.unwrap_or(0);
        obj.insert("meter_lag".to_string(), json!(lag));
    }
    // PRD-mcphost-schedules P0 requirement 5 (AC8): the scheduler tick's own
    // liveness -- `scheduler_last_tick_unix` should be within the last 60s
    // of any healthy tick loop (30s cadence), and `schedules_enabled` is a
    // live count, not a cached one.
    if let Some(obj) = body.as_object_mut() {
        obj.insert(
            "scheduler_last_tick_unix".to_string(),
            json!(state.scheduler.last_tick_unix()),
        );
        obj.insert(
            "schedules_enabled".to_string(),
            json!(state.db.count_enabled_schedule_triggers().await.unwrap_or(0)),
        );
    }
    // PRD-mcphost-inbound-events requirement 6 (AC10): host-wide inbound
    // event traffic over the trailing hour, from the same in-memory
    // sliding-window counters `POST /hooks/...` itself increments.
    if let Some(obj) = body.as_object_mut() {
        obj.insert(
            "events_received_1h".to_string(),
            json!(state.event_counters.received_1h()),
        );
        obj.insert(
            "events_rejected_1h".to_string(),
            json!(state.event_counters.rejected_1h()),
        );
    }
    // PRD-mcphost-data-retention P1 requirement 6 (AC9): `true` (nothing
    // has failed yet) until the first prune cycle that errors.
    if let Some(obj) = body.as_object_mut() {
        obj.insert(
            "last_prune_ok".to_string(),
            json!(state.db.last_prune_ok().await.unwrap_or(true)),
        );
    }
    // PRD-mcphost-sandbox-egress-allowlist requirement 4 (AC7): one
    // `{"24h": _, "all_time": _}` pair per denial reason -- `publish_plan`
    // (control::tool_publish, AC1/AC2), `run_plan` (a non-pro tenant's
    // existing public/egress tool, AC6), `run_no_proxy` (a pro tenant with
    // no `$MCPHOST_EGRESS_PROXY` configured, AC3) -- so an operator can see
    // denials (this PRD's Goals) without a direct `sqlite3` query.
    if let Some(obj) = body.as_object_mut() {
        let since = crate::state::now_unix() - 86_400;
        let mut network_denied = serde_json::Map::new();
        for reason in ["publish_plan", "run_plan", "run_no_proxy"] {
            let (last_24h, all_time) =
                state.db.network_denial_counts(reason, since).await.unwrap_or((0, 0));
            network_denied.insert(
                reason.to_string(),
                json!({"24h": last_24h, "all_time": all_time}),
            );
        }
        obj.insert("network_denied".to_string(), Value::Object(network_denied));
    }
    // PRD-mcphost-human-claim-magic-link requirement 5 / AC6: whether the
    // claim flow can actually send a magic link on this host --
    // unconfigured is a supported, non-error state (claim pages still
    // render), so the operator needs a direct signal rather than having to
    // infer it from an absence of claim traffic.
    if let Some(obj) = body.as_object_mut() {
        obj.insert(
            "claim_email_configured".to_string(),
            json!(state.email_config.is_configured()),
        );
    }
    // requirement 6 / AC9: same `{external, synthetic}` shape as
    // `tenants`/`signups`/`calls` above, restricted to claimed tenants.
    if let Some(obj) = body.as_object_mut() {
        let (claimed_external, claimed_synthetic) =
            state.db.count_claimed_tenants_by_origin().await.unwrap_or((0, 0));
        obj.insert(
            "tenants_claimed".to_string(),
            json!({"external": claimed_external, "synthetic": claimed_synthetic}),
        );
    }
    // PRD-mcphost-abuse-guard-ban-list requirement 7 / AC11.
    if let Some(obj) = body.as_object_mut() {
        let (active, auto_active, hits_24h) = state.db.ban_healthz_counts().await.unwrap_or((0, 0, 0));
        obj.insert(
            "bans".to_string(),
            json!({"active": active, "auto_active": auto_active, "hits_24h": hits_24h}),
        );
    }
    // PRD-mcphost-sqlite-busy-timeout-audit requirement 4 / AC8: summed
    // across every `db::DbRole` (`admin.db.stats`, AC7, has the per-role
    // breakdown) -- `busy_total`/`locked_total` add, `wait_max_ms` takes
    // the worst wait observed by any role.
    if let Some(obj) = body.as_object_mut() {
        let mut busy_total = 0u64;
        let mut locked_total = 0u64;
        let mut wait_max_ms = 0u64;
        for role in crate::db::ALL_ROLES {
            let c = state.db.counters(role);
            busy_total += c.busy_total;
            locked_total += c.locked_total;
            wait_max_ms = wait_max_ms.max(c.wait_max_ms);
        }
        obj.insert(
            "db".to_string(),
            json!({"busy_total": busy_total, "locked_total": locked_total, "wait_max_ms": wait_max_ms}),
        );
    }
    // PRD-mcphost-alerting-webhook requirement 6 / AC8, AC10: `open` is
    // "not yet acknowledged" (same definition `Db::alert_healthz_counts`
    // uses), `last_raised_at` is `null` on a host with no alerts yet, and
    // `sink` is purely a function of which delivery sinks are configured
    // (`AlertConfig::sink_label`) -- independent of `MCPHOST_ALERT_MIN_SEVERITY`
    // filtering any individual alert's own delivery.
    if let Some(obj) = body.as_object_mut() {
        let (open, last_raised_at) = state.db.alert_healthz_counts().await.unwrap_or((0, None));
        obj.insert(
            "alerts".to_string(),
            json!({
                "open": open,
                "last_raised_at": last_raised_at,
                "sink": state.alert_config.sink_label(),
            }),
        );
    }
    Json(body).into_response()
}

/// `POST /billing/webhook` (AC6/AC7/AC8/AC10): verifies `Stripe-Signature`
/// and applies the event via `billing::process_webhook`. Deliberately
/// takes the raw `Bytes` body (not a `Json<Value>` extractor) because
/// signature verification is over the *exact* bytes Stripe sent, before
/// any re-serialization could change them.
async fn billing_webhook(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let signature = headers
        .get("Stripe-Signature")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    match crate::billing::process_webhook(&state, &body, signature).await {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(err) => {
            let data = err.into_error_data();
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error_code": "invalid_webhook_signature", "error": data.message})),
            )
                .into_response()
        }
    }
}

/// `GET /billing/done` / `GET /billing/cancel` (requirement: "plain text
/// pages"): the `success_url`/`cancel_url` a Checkout Session redirects a
/// human's browser to after they finish (or abandon) payment. This host
/// has no UI (Non-goals: "A pricing page or any UI"), so these are the
/// whole experience -- a short, honest sentence, not a redirect back into
/// the MCP surface a browser can't call anyway.
async fn billing_done() -> impl IntoResponse {
    (
        [("Content-Type", "text/plain; charset=utf-8")],
        "Payment received. Ask your agent to call billing.status to confirm the upgrade.",
    )
}

async fn billing_cancel() -> impl IntoResponse {
    (
        [("Content-Type", "text/plain; charset=utf-8")],
        "Checkout canceled. No changes were made to your plan.",
    )
}

/// AC19: `GET /.well-known/mcp/<namespace>/server.json`, unauthenticated
/// (this is a public discovery document by design, same as the registry
/// entry it mirrors). 404 if that tenant has never run
/// `host.registry_publish` successfully.
async fn well_known_server_json(
    State(state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
) -> impl IntoResponse {
    match state.db.get_registry_document(namespace).await {
        Ok(Some(doc)) => (StatusCode::OK, Json(doc)).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error_code": "tool_not_found", "error": "no published server.json for this namespace"})),
        )
            .into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error_code": "storage", "error": "storage error"})),
        )
            .into_response(),
    }
}

/// PRD-mcphost-oauth-resource-server requirement 1 / AC1: `GET
/// /.well-known/oauth-protected-resource`, unauthenticated (RFC 9728's own
/// discovery contract) -- always 200; an empty `authorization_servers: []`
/// is the no-issuers-registered state, not an error.
async fn well_known_oauth_protected_resource(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match crate::oauth::protected_resource_metadata(&state).await {
        Ok(doc) => (StatusCode::OK, Json(doc)).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error_code": "storage", "error": "storage error"})),
        )
            .into_response(),
    }
}

/// PRD-mcphost-sharing requirement 5 (AC7): `GET /.well-known/mcp/catalog.json`,
/// unauthenticated (same public-discovery-document rationale as
/// `well_known_server_json` above) -- mirrors `host.catalog.search`'s
/// unfiltered listing for crawlers. Always 200 (an empty `tools: []` is not
/// an error).
async fn well_known_catalog(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match crate::sharing::catalog_document(&state).await {
        Ok(doc) => (StatusCode::OK, Json(doc)).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error_code": "storage", "error": "storage error"})),
        )
            .into_response(),
    }
}

/// PRD-mcphost-status-feed requirement 3/AC1/AC10: `GET /status.json`,
/// anonymous, cacheable 60s. With `component`/`days` query params both
/// present (AC10), returns that one component's daily rollup rows instead
/// of the whole feed.
async fn status_json_route(
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let by_component = params
        .get("component")
        .cloned()
        .zip(params.get("days").and_then(|d| d.parse::<i64>().ok()));

    let body = match by_component {
        Some((component, days)) => crate::statusfeed::daily_rows(&state, &component, days).await,
        None => crate::statusfeed::status_json(&state).await,
    };

    let mut response = match body {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error_code": "storage", "error": "storage error"})),
        )
            .into_response(),
    };
    if let Ok(value) = HeaderValue::from_str(&format!(
        "max-age={}",
        crate::statusfeed::CACHE_MAX_AGE_SECS
    )) {
        response.headers_mut().insert(CACHE_CONTROL, value);
    }
    response
}

/// Ensures every response carries `MCP-Protocol-Version` (AC1), and emits
/// one structured request-line log entry. This layer is attached to the
/// whole router (see `build_router` below), not just `/mcp` -- `/healthz`
/// and `/.well-known/mcp/{ns}/server.json` get the header and the log line
/// too. That is deliberate: it keeps one middleware as the single source
/// of the request log, and an unauthenticated health/discovery endpoint
/// carrying an accurate protocol-version header is harmless.
async fn protocol_version_and_log(req: Request<Body>, next: Next) -> Response {
    let start = Instant::now();
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let mcp_name = req
        .headers()
        .get("Mcp-Name")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let mut response = next.run(req).await;

    if !response.headers().contains_key("MCP-Protocol-Version")
        && let Some(value) = advertised_protocol_version()
    {
        response
            .headers_mut()
            .insert("MCP-Protocol-Version", value.clone());
    }

    let duration_ms = start.elapsed().as_millis();
    tracing::info!(
        method = %method,
        path = %path,
        mcp_name = mcp_name.as_deref().unwrap_or(""),
        duration_ms,
        status = response.status().as_u16(),
        "http request"
    );

    response
}

/// PRD-mcphost-oauth-resource-server requirement 2 / AC3-4: re-surfaces
/// `/mcp`'s JSON-RPC-level auth rejections that mean "no usable credential
/// was presented" (`tenant_key_missing` -- the code an already-anonymous
/// caller of a tenant-requiring tool gets today, AC4; this PRD's own
/// `invalid_token`, AC3) as a real HTTP 401 carrying `WWW-Authenticate:
/// Bearer resource_metadata="..."`, per the MCP authorization spec.
/// `call_tool`'s own JSON-RPC error body is untouched -- this only
/// upgrades the wrapping HTTP status and adds the header, so every
/// existing test that reads the JSON-RPC body regardless of HTTP status
/// (AC10) keeps passing unchanged. A no-op for every route but `/mcp`
/// (`signup`/`host.quickstart`/`billing.plans`/`host.redeem` never reach
/// `tenant_key_missing` in the first place -- see `handler::call_tool`'s
/// own match-arm ordering -- so this never fires for them).
async fn oauth_401_upgrade(State(state): State<Arc<AppState>>, req: Request<Body>, next: Next) -> Response {
    if req.uri().path() != "/mcp" {
        return next.run(req).await;
    }
    let response = next.run(req).await;
    let (parts, body) = response.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, usize::MAX).await else {
        return Response::from_parts(parts, Body::empty());
    };
    let error_code = serde_json::from_slice::<Value>(&bytes)
        .ok()
        .and_then(|v| v.get("error").cloned())
        .and_then(|e| e.get("data").cloned())
        .and_then(|d| d.get("error_code").cloned())
        .and_then(|c| c.as_str().map(str::to_string));
    let mut response = Response::from_parts(parts, Body::from(bytes));
    if matches!(error_code.as_deref(), Some("tenant_key_missing") | Some("invalid_token")) {
        *response.status_mut() = StatusCode::UNAUTHORIZED;
        let url = format!("{}/.well-known/oauth-protected-resource", state.public_url);
        if let Ok(value) = HeaderValue::from_str(&format!("Bearer resource_metadata=\"{url}\"")) {
            response
                .headers_mut()
                .insert(axum::http::header::WWW_AUTHENTICATE, value);
        }
    }
    response
}

pub fn build_router(state: Arc<AppState>) -> Router {
    let config = StreamableHttpServerConfig::default()
        .with_json_response(true)
        .with_legacy_session_mode(false)
        .with_max_request_body_bytes(MAX_REQUEST_BODY_BYTES)
        // This endpoint's security boundary is the bearer key, not the Host
        // header: it is meant to be reached at whatever public domain the
        // operator maps to it (see the PRD's "Public domain and TLS name"
        // open question), often behind a reverse proxy. DNS-rebinding
        // protection via allowed_hosts is the right default for a
        // loopback dev server; it is not a substitute for auth here, so we
        // disable it rather than requiring the operator to enumerate every
        // hostname up front.
        .disable_allowed_hosts();

    let mcp_state = state.clone();
    let service = StreamableHttpService::new(
        move || Ok(McpHostHandler::new(mcp_state.clone())),
        Arc::new(LocalSessionManager::default()),
        config,
    );

    Router::new()
        .route("/healthz", get(healthz))
        .route("/status.json", get(status_json_route))
        .route(
            "/.well-known/mcp/{namespace}/server.json",
            get(well_known_server_json),
        )
        .route("/.well-known/mcp/catalog.json", get(well_known_catalog))
        .route(
            "/.well-known/oauth-protected-resource",
            get(well_known_oauth_protected_resource),
        )
        .route("/billing/webhook", post(billing_webhook))
        .route("/billing/done", get(billing_done))
        .route("/billing/cancel", get(billing_cancel))
        // PRD-mcphost-inbound-events P0 requirement 1: the Caddy catch-all
        // in mcphost-deploy already proxies any unmatched path to this
        // backend, so `/hooks/...` needs no deploy-side change -- only this
        // route.
        .route("/hooks/{namespace}/{tool}", post(crate::hooks::hook_receive))
        // PRD-mcphost-webhook-inbox P0 requirement 2: the opaque-id
        // webhook-trigger route, alongside `/hooks/...` above -- distinct
        // path (`/hook/`, singular, no namespace/tool segments) since a
        // webhook trigger is addressed by its own `hook_id`, not
        // `(namespace, tool)`.
        .route("/hook/{id}", post(crate::webhooks::hook_receive))
        // PRD-mcphost-tenant-data-export P0 requirement 2: the signed
        // download URL a completed `host.export` run's result carries --
        // see `export::download`.
        .route("/exports/{run_id}", get(crate::export::download))
        // PRD-mcphost-human-claim-magic-link requirements 2-3: the claim
        // flow's own three plain-HTML routes -- `/claim/verify/{code}` is
        // registered alongside `/claim/{token}` without ambiguity since
        // matchit resolves by segment count/literal-first, same as
        // `/exports/{run_id}` alongside every other top-level route here.
        .route("/claim/{token}", get(crate::claim::get_claim).post(crate::claim::post_claim))
        .route("/claim/verify/{code}", get(crate::claim::get_verify))
        .route_service("/mcp", service)
        .layer(middleware::from_fn(protocol_version_and_log))
        .layer(middleware::from_fn_with_state(state.clone(), oauth_401_upgrade))
        .with_state(state)
}

/// Serve on an already-bound listener. Split out from [`serve`] so
/// integration tests can bind to `127.0.0.1:0`, read back the assigned
/// port, and hand the listener here without a bind-then-race.
pub async fn serve_on_listener(
    listener: tokio::net::TcpListener,
    state: Arc<AppState>,
) -> anyhow::Result<()> {
    let app = build_router(state);
    tracing::info!(addr = ?listener.local_addr().ok(), "mcphost listening");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

pub async fn serve(bind: SocketAddr, state: Arc<AppState>) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(bind).await?;
    serve_on_listener(listener, state).await
}

/// Fd 3 -- `$LISTEN_FDS_START` in the systemd socket-activation convention.
const LISTEN_FDS_START: std::os::fd::RawFd = 3;

/// PRD-mcphost-checkcompat-port-race requirement 1: accept a listener this
/// process didn't bind itself, via the systemd socket-activation
/// convention (`$LISTEN_FDS=1`, `$LISTEN_PID=<this process's pid>`) --
/// `compat_check::spawn_previous` is the first (and so far only) caller
/// that starts mcphost this way, dup2'ing an already-bound TCP listener
/// onto fd 3 before exec instead of handing this process a port to bind.
/// `None` when neither var is set (the ordinary case), or when `LISTEN_PID`
/// names a different process (someone else's inherited fds, not ours to
/// take).
fn inherited_listener() -> Option<std::net::TcpListener> {
    if std::env::var("LISTEN_FDS").ok()?.trim() != "1" {
        return None;
    }
    if let Ok(want_pid) = std::env::var("LISTEN_PID") {
        let want_pid: u32 = want_pid.trim().parse().ok()?;
        if want_pid != std::process::id() {
            return None;
        }
    }
    // SAFETY: fd 3 is the convention's first inherited descriptor; a caller that set LISTEN_FDS/LISTEN_PID this way has already dup2'd a real, bound TCP listener there and cleared CLOEXEC on it (dup2's own fd never inherits the source fd's CLOEXEC flag).
    Some(unsafe { std::net::TcpListener::from_raw_fd(LISTEN_FDS_START) })
}

/// Requirement 6: logs whether the socket was inherited or bound by
/// address. Requirement 4: `bind` (from `$MCPHOST_BIND` or the default)
/// still wins when no socket was inherited.
pub async fn serve_configured(bind: SocketAddr, state: Arc<AppState>) -> anyhow::Result<()> {
    if let Some(std_listener) = inherited_listener() {
        std_listener.set_nonblocking(true)?;
        let listener = tokio::net::TcpListener::from_std(std_listener)?;
        tracing::info!(
            addr = ?listener.local_addr().ok(),
            "mcphost: socket inherited via LISTEN_FDS"
        );
        return serve_on_listener(listener, state).await;
    }
    tracing::info!(%bind, "mcphost: binding by address");
    serve(bind, state).await
}
