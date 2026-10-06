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
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, Method, Request, StatusCode, header::CACHE_CONTROL};
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
        // PRD-mcphost-first-hour-support-surface requirement 8:
        // `help_url_served{code}` -- how many times each generated
        // `/help/<code>` page has actually been served, since this process
        // started (admin-only, same bucket as the other internal counts
        // above -- not something an anonymous caller needs).
        obj.insert("help_url_served".to_string(), crate::help::help_hits_snapshot());
        // PRD-mcphost-tool-naming-convention-and-aliases requirement 5
        // (AC5): `tool_alias_calls{tool}` -- `docs/metrics.md`'s own name
        // for this counter -- per canonical name that has seen an alias
        // and/or canonical call since this process started.
        obj.insert("tool_alias_calls".to_string(), crate::tool_aliases::alias_metrics_snapshot());
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
    // PRD-mcphost-chain-run-lineage P1 requirement 9 (AC12): additive,
    // always present (0 on a box with no composed chain runs yet) --
    // `composition_children_total` is every composed step run across every
    // tenant, `composition_parents_failed_input_total` is every chain call
    // refused before step 1 for a missing `$.input.*` value (requirement 3).
    if let Some(obj) = body.as_object_mut() {
        let composition_children_total = state.db.count_composition_children_total().await.unwrap_or(0);
        let composition_parents_failed_input_total =
            state.db.count_composition_parents_failed_input_total().await.unwrap_or(0);
        obj.insert(
            "runs".to_string(),
            json!({
                "composition_children_total": composition_children_total,
                "composition_parents_failed_input_total": composition_parents_failed_input_total,
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
    // PRD-mcphost-event-trigger-self-test P1 requirement 7 (AC10): the
    // strict-path rejection rate for `host.trigger.test` on `kind="event"`
    // triggers -- previously only visible per-row in the `runs` ledger.
    if let Some(obj) = body.as_object_mut() {
        obj.insert(
            "triggers".to_string(),
            json!({
                "event_self_tests_total": state.event_counters.self_tests_total(),
                "event_self_test_signature_invalid_total": state.event_counters.self_test_signature_invalid_total(),
            }),
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
    // PRD-mcphost-plan-catalog-state-quota-defaults requirement 3 / AC4:
    // the loaded catalog's effective quotas, so the instrument can assert a
    // quota is non-zero before scoring a stateful task without reading the
    // box's own `plans.toml`.
    // PRD-mcphost-oauth-demand-signal requirement 2 (AC2/AC5): the cached
    // per-credential-method call/tenant counts -- see
    // `oauth_stats::healthz_json`'s own doc comment for the cache contract.
    if let Some(obj) = body.as_object_mut() {
        obj.insert("oauth".to_string(), crate::oauth_stats::healthz_json(state).await);
    }
    if let Some(obj) = body.as_object_mut() {
        obj.insert(
            "plans".to_string(),
            json!(
                state
                    .plans
                    .plans
                    .iter()
                    .map(|p| json!({
                        "name": p.name,
                        "state_bytes_max": p.state_bytes_max,
                        "state_rows_max": p.state_rows_max,
                        "table_bytes_max": p.table_bytes_max,
                        "docs_bytes_max": p.docs_bytes_max,
                        "jobs_concurrent": p.jobs_concurrent,
                        "end_users_max": p.end_users_max,
                    }))
                    .collect::<Vec<_>>()
            ),
        );
    }
    // PRD-mcphost-activation-funnel requirement 4 (AC4): the external-class
    // stage counts over the trailing 7 days, next to `signups_by_source` --
    // no synthetic (non-`external`) tenant is ever counted in it.
    if let Some(obj) = body.as_object_mut() {
        obj.insert(
            "funnel_7d".to_string(),
            crate::admin::funnel_7d_external(state).await.unwrap_or_else(|_| json!({})),
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

/// PRD-mcphost-hosted-authorization-server AC1: `GET
/// /.well-known/oauth-authorization-server` and `GET
/// /.well-known/openid-configuration` -- the same RFC 8414 document at
/// both paths (Non-goals rules out id_tokens/userinfo, so this deployment
/// needs no OIDC-only fields the RFC 8414 shape doesn't already have).
async fn well_known_authorization_server(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(crate::authz::authorization_server_metadata(&state))
}

/// PRD-mcphost-enterprise-managed-auth requirement 2 (AC5): the per-tenant
/// twin of `well_known_authorization_server` above, at both
/// `/.well-known/oauth-authorization-server/t/{ns}/mcp` and
/// `/.well-known/openid-configuration/t/{ns}/mcp` -- unknown `<ns>` is 404
/// (never enumerates which namespaces exist).
async fn well_known_authorization_server_tenant(
    State(state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
) -> impl IntoResponse {
    let tenant = match state.db.find_tenant_by_namespace(namespace).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error_code": "tenant_not_found", "error": "no such tenant"})),
            )
                .into_response();
        }
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error_code": "storage", "error": "storage error"})),
            )
                .into_response();
        }
    };
    match crate::authz::authorization_server_metadata_for_tenant(&state, &tenant).await {
        Ok(doc) => (StatusCode::OK, Json(doc)).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error_code": "storage", "error": "storage error"})),
        )
            .into_response(),
    }
}

/// AC1 / requirement 1: `GET /.well-known/jwks.json` -- this host's own
/// authorization-server signing key, published for verifiers of hosted
/// access tokens (`kid` matches the header every minted token carries).
async fn well_known_authz_jwks(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(state.authz_key.jwks_document())
}

/// PRD-mcphost-tenant-resource-metadata requirement 2 / AC1: `GET
/// /.well-known/oauth-protected-resource/t/{ns}/mcp` -- the per-tenant
/// twin of [`well_known_oauth_protected_resource`] above, unauthenticated
/// (same RFC 9728 discovery contract). An unknown `<ns>` is 404 (never
/// enumerates which namespaces exist by distinguishing "unknown" from any
/// other error).
async fn well_known_oauth_protected_resource_tenant(
    State(state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
) -> impl IntoResponse {
    let tenant = match state.db.find_tenant_by_namespace(namespace).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error_code": "tenant_not_found", "error": "no such tenant"})),
            )
                .into_response();
        }
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error_code": "storage", "error": "storage error"})),
            )
                .into_response();
        }
    };
    match crate::oauth::protected_resource_metadata_for_tenant(&state, &tenant).await {
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

/// mcphost-polish-p0-20260930 (audit finding 1): README's own "Connect"
/// checklist links `/status.html` and `/aup.html`, but neither was ever
/// routed by this binary -- a comment on the `boa_engine` dev-dependency
/// (Cargo.toml) assumed "a static file only mcphost-deploy's Caddy ever
/// serves" would cover it. That assumption was wrong in prod (both 404'd
/// live): the Caddy catch-all documented above (`/hooks/...`'s own comment)
/// already proxies every unmatched path to this backend, so a Caddy config
/// drift (or simply never having a static-file block for `www/`) means
/// this binary's own 404 is what a visitor actually sees. Embedding the
/// page text at compile time and routing it here removes the
/// deploy-config's static-file block as a second place these two pages'
/// uptime depends on.
/// Generalized by PRD-mcphost-first-hour-support-surface (non-functional:
/// "no duplicating" the status/aup pattern) into one helper every
/// compile-time-embedded `www/*.html` page's route closes over, instead of
/// a new named `async fn` per page -- `status.html`/`aup.html` keep the
/// exact route/response shape the hotfix above gave them (still
/// `include_str!`, still `claim::html_response`), `plans.html` (new,
/// requirement 3) joins them the same way.
fn static_page(content: &'static str) -> Response {
    crate::claim::html_response(StatusCode::OK, content.to_string())
}

/// Twin of [`static_page`] for the plain-text `www/*.txt` files
/// (`llms.txt`'s own convention expects `text/plain`, not HTML).
fn static_text_page(content: &'static str) -> Response {
    (
        StatusCode::OK,
        [("content-type", "text/plain; charset=utf-8")],
        content,
    )
        .into_response()
}

/// Twin of [`static_text_page`] for `www/skill.md` (requirement 4,
/// support_ac06): README/llms.txt now link `/skill.md` same-host, and the
/// `Content-Type` header is `text/markdown`, not `text/plain` -- an agent
/// or client reading it should see it as the markdown it is, matching how
/// mcphost-deploy's own Caddy route serves it in prod.
fn static_markdown_page(content: &'static str) -> Response {
    (
        StatusCode::OK,
        [("content-type", "text/markdown; charset=utf-8")],
        content,
    )
        .into_response()
}

/// `GET /help/<code>` (requirement 2, AC1-AC2): rendered at request time
/// from `help::HELP_ENTRIES` plus whatever `MCPHOST_SUPPORT_URL` this
/// process has *right now* -- not a committed file -- so an operator
/// setting that env var takes effect without a rebuild (see `help.rs`'s
/// module doc comment for why this one page and `support.html`/`/help`
/// below are request-time-rendered while `status.html`/`aup.html`/
/// `plans.html` stay compile-time-embedded).
async fn help_page(Path(code): Path<String>) -> Response {
    match crate::help::entry(&code) {
        Some(e) => {
            crate::help::record_help_served(e.code);
            let support_url = std::env::var("MCPHOST_SUPPORT_URL").ok();
            crate::claim::html_response(
                StatusCode::OK,
                crate::help::render_help_page(e, support_url.as_deref()),
            )
        }
        None => crate::claim::html_response(
            StatusCode::NOT_FOUND,
            "no help page for that code".to_string(),
        ),
    }
}

/// `GET /help` (requirement 7's `links.help`): an index of every code, so
/// that link resolves to something readable rather than a 404.
async fn help_index() -> Response {
    let support_url = std::env::var("MCPHOST_SUPPORT_URL").ok();
    crate::claim::html_response(
        StatusCode::OK,
        crate::help::render_help_index(support_url.as_deref()),
    )
}

/// `GET /support.html` (requirement 4): see [`help_page`]'s doc comment for
/// why this is request-time-rendered rather than a committed file.
async fn support_page() -> Response {
    let support_url = std::env::var("MCPHOST_SUPPORT_URL").ok();
    crate::claim::html_response(
        StatusCode::OK,
        crate::help::render_support_page(support_url.as_deref()),
    )
}

/// `GET /plans.json` (requirement 3, AC4): the exact `billing::plans`
/// output -- the same function the `billing.plans` tool calls, so the two
/// can never disagree.
async fn plans_json_route(State(state): State<Arc<AppState>>) -> Response {
    (StatusCode::OK, Json(crate::billing::plans(&state))).into_response()
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
/// (AC10) keeps passing unchanged.
///
/// PRD-mcphost-tenant-resource-metadata requirement 1/4 (AC2-5): also
/// covers `/t/{ns}/mcp`, naming that tenant's own metadata URL instead of
/// the root document's, and upgrades `wrong_tenant` the same way
/// `invalid_token` already was; `insufficient_scope` becomes a real HTTP
/// 403 with the RFC 6750 §3 `error="insufficient_scope"` challenge. Every
/// 401/403 challenge this middleware writes now also carries `scope="mcp"`
/// (spec 2026-07-28 requirement 4). A no-op for every other route
/// (`signup`/`host.quickstart`/`billing.plans`/`host.redeem` never reach
/// `tenant_key_missing` in the first place -- see `handler::call_tool`'s
/// own match-arm ordering -- so this never fires for them).
async fn oauth_401_upgrade(State(state): State<Arc<AppState>>, req: Request<Body>, next: Next) -> Response {
    let path = req.uri().path();
    let metadata_url = if path == "/mcp" {
        format!("{}/.well-known/oauth-protected-resource", state.public_url)
    } else if let Some(ns) = path.strip_prefix("/t/").and_then(|s| s.strip_suffix("/mcp")) {
        crate::oauth::tenant_metadata_url(&state.public_url, ns)
    } else {
        return next.run(req).await;
    };
    let response = next.run(req).await;
    let (parts, body) = response.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, usize::MAX).await else {
        return Response::from_parts(parts, Body::empty());
    };
    let data = serde_json::from_slice::<Value>(&bytes)
        .ok()
        .and_then(|v| v.get("error").cloned())
        .and_then(|e| e.get("data").cloned());
    let error_code = data.as_ref().and_then(|d| d.get("error_code")).and_then(Value::as_str).map(str::to_string);
    // PRD-mcphost-tool-scopes-and-consent requirement 4 (AC2): a per-tool
    // `insufficient_scope` refusal names the union scope in `data.scope`;
    // the pre-existing blanket `AppError::InsufficientScope` carries no
    // such field, so `mcp` (its own, unchanged, only possible value) is
    // still the fallback.
    let scope = data
        .as_ref()
        .and_then(|d| d.get("scope"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| "mcp".to_string());
    let mut response = Response::from_parts(parts, Body::from(bytes));
    match error_code.as_deref() {
        Some("tenant_key_missing") | Some("invalid_token") | Some("wrong_tenant") => {
            *response.status_mut() = StatusCode::UNAUTHORIZED;
            if let Ok(value) =
                HeaderValue::from_str(&format!("Bearer resource_metadata=\"{metadata_url}\", scope=\"mcp\""))
            {
                response
                    .headers_mut()
                    .insert(axum::http::header::WWW_AUTHENTICATE, value);
            }
        }
        Some("insufficient_scope") => {
            *response.status_mut() = StatusCode::FORBIDDEN;
            if let Ok(value) = HeaderValue::from_str(&format!(
                "Bearer error=\"insufficient_scope\", scope=\"{scope}\", resource_metadata=\"{metadata_url}\""
            )) {
                response
                    .headers_mut()
                    .insert(axum::http::header::WWW_AUTHENTICATE, value);
            }
        }
        _ => {}
    }
    response
}

/// PRD-mcphost-url-bound-tenants requirement 4: `GET /u/new` -- a one-button
/// page; pressing it `POST`s to the same path (see [`post_new_url`]).
async fn get_new_url() -> Response {
    crate::claim::html_response(StatusCode::OK, render_new_url_page())
}

fn render_new_url_page() -> String {
    crate::claim::page(
        "mcphost — get your URL",
        "<h1>Get your URL</h1>\
         <p>One click gets you a private mcphost URL -- no account, no email. Paste it into \
         Claude Code, Claude Desktop, claude.ai, or Cursor, and your agent's next message can \
         publish a tool.</p>\
         <form method=\"post\" action=\"/u/new\"><button type=\"submit\">Get my URL</button></form>",
    )
}

/// requirement 4 / AC4: the literal phrase every rate-limited `POST /u/new`
/// gets -- same page claim.rs's own `render_rate_limited` renders for
/// `/claim/*`, text-identical since the signup limiter is the thing that
/// actually fired either way, but a copy here rather than a cross-module
/// call since the two modules render independent pages.
fn render_new_url_rate_limited() -> String {
    crate::claim::page(
        "mcphost — too many requests",
        "<h1>Too many requests</h1><p>Try again later.</p>",
    )
}

fn render_new_url_storage_error() -> String {
    crate::claim::page(
        "mcphost — something went wrong",
        "<h1>Something went wrong</h1><p>Please try again in a minute.</p>",
    )
}

/// requirement 4 (AC4): the four copy snippets the page shows once a URL
/// exists -- Claude Code, Claude Desktop/claude.ai (paste), Cursor (JSON),
/// and generic JSON, in that order.
fn render_new_url_result_page(url: &str) -> String {
    let url = crate::claim::html_escape(url);
    crate::claim::page(
        "mcphost — your URL",
        &format!(
            "<h1>Your mcphost URL</h1>\
             <p><code>{url}</code></p>\
             <p>Treat this link like a password -- anyone who has it can act as your tenant. \
             If it ever leaks, ask your agent to call <code>host.key_rotate</code> to mint a \
             fresh one.</p>\
             <p>Claude Code:</p><p><code>claude mcp add --transport http mcphost {url}</code></p>\
             <p>Claude Desktop / claude.ai: paste this URL into \"Add custom connector\".</p>\
             <p>Cursor (<code>mcp.json</code>):</p>\
             <p><code>{{&quot;mcpServers&quot;: {{&quot;mcphost&quot;: {{&quot;url&quot;: &quot;{url}&quot;}}}}}}</code></p>\
             <p>Generic JSON:</p><p><code>{{&quot;url&quot;: &quot;{url}&quot;}}</code></p>",
        ),
    )
}

/// `POST /u/new` (requirement 4, AC4): mints a tenant via the existing
/// `signup` path (`source: "url-page"`), subject to the same per-IP limiter
/// and kill switch as any other signup, then mints and shows that tenant's
/// own URL secret -- its first one, since this tenant was just created.
async fn post_new_url(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Response {
    let ip = crate::claim::source_ip(&headers, peer);
    let display_name = format!("agent-{}", crate::state::new_ulid());
    let signup_result = crate::control::signup(
        &state,
        &json!({"name": display_name, "source": "url-page"}),
        &ip,
        crate::control::SignupAttribution::default(),
    )
    .await;
    let tenant_id = match signup_result {
        Ok(value) => {
            let Some(namespace) = value.get("tenant").and_then(Value::as_str) else {
                return crate::claim::html_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    render_new_url_storage_error(),
                );
            };
            match state.db.find_tenant_by_namespace(namespace.to_string()).await {
                Ok(Some(tenant)) => tenant.id,
                _ => {
                    return crate::claim::html_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        render_new_url_storage_error(),
                    );
                }
            }
        }
        Err(err) if err.code() == "rate_limited" => {
            return crate::claim::html_response(
                StatusCode::TOO_MANY_REQUESTS,
                render_new_url_rate_limited(),
            );
        }
        Err(_) => {
            return crate::claim::html_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                render_new_url_storage_error(),
            );
        }
    };
    let url_secret = crate::auth::generate_url_secret();
    if state
        .db
        .rotate_tenant_url_secret(tenant_id, crate::auth::hash_key(&url_secret))
        .await
        .is_err()
    {
        return crate::claim::html_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            render_new_url_storage_error(),
        );
    }
    let url = format!("{}/u/{}/mcp", state.public_url.trim_end_matches('/'), url_secret);
    crate::claim::html_response(StatusCode::OK, render_new_url_result_page(&url))
}

/// PRD-mcphost-url-bound-tenants requirement 1/5 (AC2, AC5): `/u/{secret}/mcp`
/// for a secret that names no tenant at all is 404, byte-identical to any
/// other unmapped path -- same shape as [`require_known_tenant_namespace`]
/// below, scoped to this route instead. A known secret's own `GET` without
/// MCP's required `Accept` header (i.e. a browser navigating straight to
/// the link, the way a human who just pasted it from `/u/new` would) gets a
/// short text/html explainer instead of ever reaching the MCP service
/// (AC5) -- so a client never gets a chance to attempt an MCP handshake, or
/// see an MCP-shaped error, against what a human just opened in a browser
/// tab. Runs before `resolve_path_secret_auth`/dispatch (this middleware
/// sits in front of the whole streamable-HTTP service), so both checks
/// apply regardless of any other credential the request carries.
async fn require_known_url_secret(
    State(state): State<Arc<AppState>>,
    Path(secret): Path<String>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let hash = crate::auth::hash_key(&secret);
    match state.db.find_tenant_by_url_secret_hash(hash).await {
        Ok(Some(_)) => {}
        _ => return StatusCode::NOT_FOUND.into_response(),
    }
    let wants_html = req
        .headers()
        .get(axum::http::header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("text/html"));
    if req.method() == Method::GET && wants_html {
        return crate::claim::html_response(StatusCode::OK, render_url_explainer(&state, &secret));
    }
    next.run(req).await
}

/// PRD-mcphost-invite-links requirement 4 (AC5): `/i/{code}/mcp` for a
/// code that is unknown, revoked, expired, or already at `max_uses` is 404
/// with error class `invite_invalid` -- same "404 before the MCP service
/// ever sees the request" shape as [`require_known_url_secret`] above,
/// scoped to this route. A browser's `GET` (no MCP `Accept` header) on a
/// LIVE code gets a short explainer that never names the inviter -- it
/// reads straight off `code` alone, never looks up (or renders) the
/// inviter's namespace/display_name.
async fn require_live_invite_code(
    State(state): State<Arc<AppState>>,
    Path(code): Path<String>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let hash = crate::auth::hash_key(&code);
    match state.db.find_invite_by_code_hash(hash).await {
        Ok(Some(invite)) if crate::db::is_connectable(&invite, crate::state::now_unix()) => {}
        _ => {
            return crate::claim::html_response(
                StatusCode::NOT_FOUND,
                crate::claim::page(
                    "mcphost — invite not valid",
                    "<h1>This invite link is no longer valid</h1>\
                     <p>It may have expired, been revoked, or already reached its use limit. \
                     Ask whoever shared it with you for a fresh one.</p>",
                ),
            );
        }
    }
    let wants_html = req
        .headers()
        .get(axum::http::header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("text/html"));
    if req.method() == Method::GET && wants_html {
        return crate::claim::html_response(
            StatusCode::OK,
            crate::claim::page(
                "mcphost — you've been invited",
                "<h1>You've been invited to mcphost</h1>\
                 <p>Add this URL as an MCP server in your client, then ask your agent to call \
                 any tool -- that first call creates your own tenant and accepts the \
                 connection, no signup step needed.</p>",
            ),
        );
    }
    next.run(req).await
}

fn render_url_explainer(state: &AppState, secret: &str) -> String {
    let url = format!("{}/u/{}/mcp", state.public_url.trim_end_matches('/'), secret);
    let url = crate::claim::html_escape(&url);
    crate::claim::page(
        "mcphost — your URL",
        &format!(
            "<h1>This is your mcphost URL</h1>\
             <p>Add <code>{url}</code> as an MCP server in your client -- no login, no key. \
             Anyone with this exact link can act as your tenant, so treat it like a password. \
             Leaked it? Ask your agent to call <code>host.key_rotate</code> for a fresh one.</p>",
        ),
    )
}

/// PRD-mcphost-tenant-resource-metadata requirement 1 (AC3): `/t/{ns}/mcp`
/// for an `<ns>` that names no tenant at all is 404, byte-identical to
/// axum's own no-route-matched fallback (`StatusCode::NOT_FOUND.into_response()`
/// -- see `axum::routing::not_found::NotFound`) for any other unmapped
/// path, so a probe learns nothing about which namespaces exist. Runs
/// before `resolve_auth`/dispatch (this middleware sits in front of the
/// whole streamable-HTTP service), so it applies regardless of credential.
async fn require_known_tenant_namespace(
    State(state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
    req: Request<Body>,
    next: Next,
) -> Response {
    match state.db.find_tenant_by_namespace(namespace).await {
        Ok(Some(_)) => next.run(req).await,
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}


/// PRD-mcphost-session-bound-tenant-after-signup requirement 1: the session
/// identity the binding map is keyed by.
///
/// The transport underneath stays stateless (`build_router` still passes
/// `with_legacy_session_mode(false)`), because `rmcp`'s own session mode
/// *requires* the header back on every follow-up request and the Claude
/// Agent SDK never sends one -- see `session_bind`'s module docs and
/// `tests/sessbind_ac01_*`. So the identifier is minted here instead:
///
/// * a request carrying an `Mcp-Session-Id` this process actually minted
///   keeps it (that is what gives a signup and a later key-less call on the
///   same connection one identity);
/// * anything else -- no header, or a header holding a value this process
///   did not mint (a guess, a client-chosen string, another process's id) --
///   is replaced by a freshly minted one before the handler ever sees it, so
///   a binding is never addressed by client input (the PRD's own
///   non-functional clause);
/// * every response echoes the session id, so a spec-compliant client can
///   send it back. The request's own headers are never rewritten -- the
///   resolved id rides in a request extension
///   ([`crate::session_bind::RequestSessionId`]) so the transport
///   underneath keeps reading the wire exactly as it did before. A client that ignores the header -- the SDK -- behaves
///   byte-for-byte as it does at v0.60.35: it never reaches a binding,
///   because it never presents the id one is stored under.
async fn issue_session_id(
    State(state): State<Arc<AppState>>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    let header = http::HeaderName::from_static(crate::session_bind::SESSION_ID_HEADER);
    let presented = req
        .headers()
        .get(&header)
        .and_then(|v| v.to_str().ok())
        .filter(|id| state.session_bindings.is_server_issued(id))
        .map(str::to_string);
    let session_id = presented.unwrap_or_else(|| state.session_bindings.issue());
    req.extensions_mut()
        .insert(crate::session_bind::RequestSessionId(session_id.clone()));
    let mut response = next.run(req).await;
    if let Ok(value) = HeaderValue::from_str(&session_id) {
        response.headers_mut().insert(header, value);
    }
    response
}

pub fn build_router(state: Arc<AppState>) -> Router {
    build_router_with_session_mode(state, false)
}

/// PRD-mcphost-session-bound-tenant-after-signup: test-only seam behind
/// `build_router`'s own fixed `false` -- production behavior is byte-for-
/// byte unchanged (`build_router` always calls this with `false`). Exists
/// so `tests/sessbind_ac01_transport_stateless_blocks_session_binding.rs`
/// can build the exact production router with `legacy_session_mode(true)`
/// and prove, against a real server, that enabling `rmcp`'s only session-
/// identity mechanism breaks the already-shipped Claude Agent SDK replay
/// (`tests/compat_ac11_ac12_claude_sdk_replay.rs`) -- the PRD's own
/// technical-considerations exit clause ("if mcphost runs the transport
/// stateless ... this PRD is blocked") turned into a reproducible test
/// rather than an assertion in a changelog.
pub fn build_router_with_session_mode(state: Arc<AppState>, legacy_session_mode: bool) -> Router {
    let config = StreamableHttpServerConfig::default()
        .with_json_response(true)
        .with_legacy_session_mode(legacy_session_mode)
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
    // PRD-mcphost-tenant-resource-metadata requirement 1: `/t/{ns}/mcp`
    // serves the identical streamable-HTTP service as `/mcp` -- the
    // path-bound tenant is read straight off the request's own URI inside
    // `handler.rs` (`resolve_auth`'s wrong-tenant check), not threaded
    // through here. Scoped to its own sub-router (rather than `.layer`-ing
    // the whole router below) so the unknown-namespace 404 guard applies
    // only to this one route.
    // `route_layer` (not `layer`): scoped to the route actually matched,
    // never to the router's own default-NotFound fallback -- `.layer()`
    // here would wrap that fallback too, and `Router::merge` can promote a
    // sub-router's (now middleware-wrapped) default fallback to the merged
    // router's own, turning every genuinely unmapped path's 404 into a
    // `Path` extraction failure (500) instead.
    //
    // PRD-mcphost-enterprise-managed-auth requirement 3 (AC2): also the
    // endpoint a per-tenant resource URI (`crate::authz::tenant_resource`)
    // needs for a JWT-bearer-grant-minted bearer to call tools on -- tenant
    // resolution for any bearer credential already comes from the bearer
    // itself (`handler::resolve_auth`), never from the URL path.
    let tenant_mcp_router = Router::new()
        .route_service("/t/{namespace}/mcp", service.clone())
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_known_tenant_namespace,
        ))
        // PRD-mcphost-session-bound-tenant-after-signup requirement 1: a
        // `/t/{ns}/mcp` connection gets a session identity on exactly the
        // same terms as `/mcp` -- see `root_mcp_router` below.
        .route_layer(middleware::from_fn_with_state(state.clone(), issue_session_id));

    // PRD-mcphost-url-bound-tenants requirement 1: `/u/{secret}/mcp` serves
    // the identical streamable-HTTP service as `/mcp`/`/t/{ns}/mcp` --
    // path-secret resolution happens inside `handler.rs`
    // (`resolve_path_secret_auth`), not here. Same `route_layer` shape as
    // `tenant_mcp_router` above, for the same "scoped to the route actually
    // matched" reason.
    let url_mcp_router = Router::new()
        .route_service("/u/{secret}/mcp", service.clone())
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_known_url_secret,
        ))
        .route_layer(middleware::from_fn_with_state(state.clone(), issue_session_id));

    // PRD-mcphost-invite-links requirement 2: `/i/{code}/mcp` serves the
    // identical streamable-HTTP service as `/mcp`/`/u/{secret}/mcp` --
    // credential/tenant-creation resolution happens inside `handler.rs`
    // (`handler::call_tool`'s own invite-claim step), not here. Same
    // `route_layer` shape as `url_mcp_router` above.
    let invite_mcp_router = Router::new()
        .route_service("/i/{code}/mcp", service.clone())
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_live_invite_code,
        ))
        .route_layer(middleware::from_fn_with_state(state.clone(), issue_session_id));

    // PRD-mcphost-session-bound-tenant-after-signup requirement 1: only the
    // streamable-HTTP routes get a session identity, so `/healthz`, the
    // OAuth endpoints and the router's own 404 fallback are untouched. Its
    // own sub-router with `route_layer` (rather than `.layer()` on the
    // merged router) for exactly the reason `tenant_mcp_router` above
    // already documents: `.layer()` would wrap the default fallback too.
    let root_mcp_router = Router::new()
        .route_service("/mcp", service)
        .route_layer(middleware::from_fn_with_state(state.clone(), issue_session_id));

    // PRD-mcphost-public-tool-url requirement 11 (AC10): its own sub-router
    // so `add_cors_header` (`route_layer`, not `.layer()`, same rationale
    // as `tenant_mcp_router`/`root_mcp_router` above) applies only to the
    // `/x/...` family, never the router's own default-NotFound fallback or
    // any other route.
    let public_tool_router = Router::new()
        .route(
            "/x/{token}/{tool}",
            get(crate::public_tool::x_get)
                .post(crate::public_tool::x_post)
                .options(crate::public_tool::x_options),
        )
        .route("/x/{token}/{tool}/runs/{run_id}", get(crate::public_tool::x_poll))
        .route_layer(middleware::from_fn(crate::public_tool::add_cors_header));

    Router::new()
        .route("/healthz", get(healthz))
        .route("/status.json", get(status_json_route))
        // mcphost-polish-p0-20260930 (audit finding 1), generalized by
        // PRD-mcphost-first-hour-support-surface: see `static_page`'s own
        // doc comment -- same compile-time-embedded `www/*.html` pattern,
        // one shared helper instead of one named `async fn` per page.
        .route("/status.html", get(|| async { static_page(include_str!("../www/status.html")) }))
        .route("/aup.html", get(|| async { static_page(include_str!("../www/aup.html")) }))
        // requirement 3 (AC4): a static shell that fetches the live
        // `/plans.json` client-side -- see `www/plans.html`'s own comment.
        .route("/plans.html", get(|| async { static_page(include_str!("../www/plans.html")) }))
        .route("/plans.json", get(plans_json_route))
        // requirement 5/8 (AC6, AC8): README/llms.txt already link
        // `/llms.txt` (absolute, as `https://mcphost.dev/llms.txt`) and
        // `/llms-full.txt` -- routed here for the same reason status/aup
        // are (requirement 8: every `www/` page is served *by this
        // binary*, not left to a reverse proxy's own static-file config to
        // get right or drift on).
        .route("/llms.txt", get(|| async { static_text_page(include_str!("../www/llms.txt")) }))
        .route("/llms-full.txt", get(|| async { static_text_page(include_str!("../www/llms-full.txt")) }))
        // PRD-mcphost-www-trust-pages requirement 4 / support_ac06: this
        // binary serves `/skill.md` itself, same as `/llms.txt` above --
        // prod's Caddy route was the only thing serving it (404'd from
        // here, the same gap status.html/aup.html's own hotfix closed).
        .route("/skill.md", get(|| async { static_markdown_page(include_str!("../www/skill.md")) }))
        // requirement 2/4 (AC1, AC2, AC7): request-time-rendered, not
        // embedded -- see `help_page`'s doc comment.
        .route("/help/{code}", get(help_page))
        .route("/help", get(help_index))
        .route("/support.html", get(support_page))
        .route(
            "/.well-known/mcp/{namespace}/server.json",
            get(well_known_server_json),
        )
        .route("/.well-known/mcp/catalog.json", get(well_known_catalog))
        .route(
            "/.well-known/oauth-protected-resource",
            get(well_known_oauth_protected_resource),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(well_known_authorization_server),
        )
        .route(
            "/.well-known/openid-configuration",
            get(well_known_authorization_server),
        )
        .route("/.well-known/jwks.json", get(well_known_authz_jwks))
        // PRD-mcphost-enterprise-managed-auth requirement 2 (AC5): per-tenant
        // twin of the root metadata document above, registered at both
        // well-known paths for the same reason the root pair is.
        .route(
            "/.well-known/oauth-authorization-server/t/{namespace}/mcp",
            get(well_known_authorization_server_tenant),
        )
        .route(
            "/.well-known/openid-configuration/t/{namespace}/mcp",
            get(well_known_authorization_server_tenant),
        )
        .route(
            "/oauth/authorize",
            get(crate::authz::get_authorize).post(crate::authz::post_authorize),
        )
        // PRD-mcphost-oauth-unverified-client-consent-warning AC7: the
        // unverified-caution's own explainer link -- static, no tenant/
        // client context, so registered alongside every other `/oauth/*`
        // route rather than needing `AppState` at all.
        .route(
            "/oauth/unverified-app",
            get(crate::authz::get_unverified_app_explainer),
        )
        .route("/oauth/register", post(crate::authz::post_register))
        .route("/oauth/token", post(crate::authz::post_token))
        .route("/oauth/revoke", post(crate::authz::post_revoke))
        .route(
            "/.well-known/oauth-protected-resource/t/{namespace}/mcp",
            get(well_known_oauth_protected_resource_tenant),
        )
        // PRD-mcphost-federated-end-user-login requirement 2: the
        // identity provider redirects here with the upstream code (`GET`);
        // the browser's own consent approval posts back to the same path
        // (`POST`).
        .route(
            "/oauth/federation/callback",
            get(crate::federation::get_callback).post(crate::federation::post_callback),
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
        // PRD-mcphost-chart-in-a-minute P1 requirement 6: the signed share
        // link a `host.table.chart {share: true}` call's result carries --
        // see `chart::download`.
        .route("/charts/{id}", get(crate::chart::download))
        // PRD-mcphost-human-claim-magic-link requirements 2-3: the claim
        // flow's own three plain-HTML routes -- `/claim/verify/{code}` is
        // registered alongside `/claim/{token}` without ambiguity since
        // matchit resolves by segment count/literal-first, same as
        // `/exports/{run_id}` alongside every other top-level route here.
        .route("/claim/{token}", get(crate::claim::get_claim).post(crate::claim::post_claim))
        .route("/claim/verify/{code}", get(crate::claim::get_verify))
        // PRD-mcphost-upstream-token-vault requirement 3: the handoff
        // route family's own two plain routes -- `/vault/connect/{token}`
        // redeems the one-time link and redirects to the provider,
        // `/vault/callback` completes the code exchange server-side.
        .route("/vault/connect/{token}", get(crate::vault::get_connect))
        .route("/vault/callback", get(crate::vault::get_callback))
        // PRD-mcphost-url-bound-tenants requirement 4: the one-button
        // signup page -- `GET` renders it, `POST` mints the tenant and its
        // first URL.
        .route("/u/new", get(get_new_url).post(post_new_url))
        .merge(root_mcp_router)
        .merge(tenant_mcp_router)
        .merge(url_mcp_router)
        .merge(invite_mcp_router)
        .merge(public_tool_router)
        // PRD-mcphost-session-bound-tenant-after-signup requirement 7
        // (AC10): `oauth_401_upgrade` must run INSIDE (closer to the
        // router than) `protocol_version_and_log` -- axum's `.layer(L)`
        // wraps the current service, so the LAST `.layer()` call becomes
        // the OUTERMOST middleware. With `oauth_401_upgrade` added last
        // (as it used to be), it saw the response before
        // `protocol_version_and_log` (added before it, so wrapped inside
        // it) had a chance to log -- the log line recorded the pre-upgrade
        // status (200 for a `tenant_key_missing` refusal actually sent to
        // the client as 401). Swapping the order makes
        // `protocol_version_and_log` the outermost layer, so it logs
        // whatever `oauth_401_upgrade` (now inner) already rewrote.
        .layer(middleware::from_fn_with_state(state.clone(), oauth_401_upgrade))
        .layer(middleware::from_fn(protocol_version_and_log))
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
