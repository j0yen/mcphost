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
use axum::http::{
    HeaderMap, HeaderValue, Method, Request, StatusCode,
    header::{CACHE_CONTROL, CONTENT_TYPE, LOCATION, STRICT_TRANSPORT_SECURITY},
};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use rmcp::model::ProtocolVersion;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use serde_json::{Value, json};

pub mod routes;

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
/// `{"ok": true}` (200) or `{"ok": false}` (503) plus, since
/// PRD-mcphost-healthz-version-field, `version` and `git_sha` and nothing
/// else -- `paying_tenants`/`tenants_total`/`tools_total`/`billing_mode`/
/// `sandbox_*`/`version` are business metrics and reconnaissance-grade
/// facts that used to leak to any unauthenticated caller. The full
/// document (unchanged shape from before this PRD) now requires the admin
/// bearer; a wrong or absent key both fall through to the same anonymous
/// body (requirement 3/AC3) since [`is_admin_request`] doesn't distinguish
/// "no header" from "wrong header".
async fn healthz(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    // PRD-mcphost-graceful-drain-contract R1: a draining process answers 503
    // `{"state":"draining","restart_id":…}` to everyone.
    if let Some(restart_id) = crate::drain::global().restart_id() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"ok": false, "state": "draining", "restart_id": restart_id})),
        )
            .into_response();
    }
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

/// PRD-mcphost-healthz-anon-keys-const requirement 2: the one builder of the
/// anonymous `/healthz` body. Its keys must equal
/// [`ANON_HEALTHZ_KEYS`](crate::build_info::ANON_HEALTHZ_KEYS); a debug build
/// panics on disagreement, and [`check_anon_healthz_keys`] is the same check
/// for release builds and for `hzkeys_ac02`.
pub fn anon_healthz_body(ok: bool, build: &crate::build_info::BuildInfo) -> serde_json::Map<String, serde_json::Value> {
    let body = anon_healthz_body_with(ok, build, |_| {});
    debug_assert_eq!(check_anon_healthz_keys(&body), Ok(()));
    body
}

/// The builder's body construction with a hook that runs on the finished map
/// and is unchecked. Production passes a no-op through
/// [`anon_healthz_body`]; the handler passes an insert when
/// `AppState::anon_healthz_extra_key` is armed (the AC2 fixture).
pub fn anon_healthz_body_with(
    ok: bool,
    build: &crate::build_info::BuildInfo,
    tweak: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>),
) -> serde_json::Map<String, serde_json::Value> {
    let mut body = serde_json::Map::new();
    body.insert("ok".into(), json!(ok));
    body.extend(build.json());
    tweak(&mut body);
    body
}

/// `Ok` when `body`'s keys equal `ANON_HEALTHZ_KEYS`; otherwise an error
/// message naming the const so the one test that fails says what to change.
pub fn check_anon_healthz_keys(body: &serde_json::Map<String, serde_json::Value>) -> Result<(), String> {
    let mut keys: Vec<&str> = body.keys().map(String::as_str).collect();
    keys.sort_unstable();
    if keys == crate::build_info::ANON_HEALTHZ_KEYS {
        Ok(())
    } else {
        Err(format!(
            "anonymous /healthz keys {keys:?} differ from ANON_HEALTHZ_KEYS {:?}; change the const and the handler together",
            crate::build_info::ANON_HEALTHZ_KEYS
        ))
    }
}

async fn healthz_response(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let db_ok = state.db.is_writable().await;
    // PRD-mcphost-data-retention requirement 4 (AC6): the disk-floor guard
    // is as much a liveness signal as `db_ok` -- a box below the floor
    // refuses every write the same way an unwritable database does, so it
    // folds into the anonymous `ok` the same way (AC14 precedent).
    let disk_ok = state.disk_guard.is_ok(state.db.data_dir());

    if !is_admin_request(state, headers) {
        // PRD-mcphost-healthz-version-field requirement 2: `ok` plus the
        // two build-identity keys from the single `BuildInfo` renderer.
        let ok = db_ok && disk_ok;
        let body = match state.anon_healthz_extra_key.get() {
            None => anon_healthz_body(ok, state.build),
            // PRD-mcphost-healthz-anon-keys-const AC2 fixture: a sibling's
            // extra key, unchecked so `hzkeys_ac02` is what reports it.
            Some(&extra) => anon_healthz_body_with(ok, state.build, |m| {
                m.insert(extra.into(), json!(true));
            }),
        };
        let status = if ok { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE };
        return (status, Json(serde_json::Value::Object(body))).into_response();
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
        "contract_version": state.contract.version,
        "contract_sha": state.contract.sha,
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
        obj.extend(state.build.json());
        // PRD-mcphost-first-tool-from-description R6: whether a draft model
        // is configured, and the last day's drafts with their token cost.
        obj.insert("draft_configured".to_string(), json!(state.drafter.is_some()));
        obj.insert(
            "drafts_24h".to_string(),
            state
                .db
                .drafts_since(crate::state::now_unix() - 86_400)
                .await
                .unwrap_or(json!({"n": 0, "ok": 0, "failed": 0, "tokens": 0})),
        );
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
        // PRD-mcphost-secret-non-exfiltration-proof R5: blocks in the last 24 h.
        obj.insert(
            "secret_leak_blocked_24h".to_string(),
            json!(state.db.secret_leak_blocks_since(since).await.unwrap_or(0)),
        );
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
    // PRD-mcphost-ownership-moment requirement 4 (AC4): the market test's
    // own number -- claims by source class and time-to-claim, over the
    // trailing 30 days (distinct from `tenants_claimed` above, which is
    // all-time and carries no nudge/median fields).
    if let Some(obj) = body.as_object_mut() {
        let since = crate::state::now_unix() - 30 * 86_400;
        let summary = state.db.claims_summary(since).await.unwrap_or_default();
        obj.insert(
            "claims".to_string(),
            json!({
                "external": summary.external,
                "synthetic": summary.synthetic,
                "median_minutes_to_claim": summary.median_minutes_to_claim,
                "nudged": summary.nudged,
                "nudged_then_claimed": summary.nudged_then_claimed,
            }),
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
                Json(json!({"error_code": crate::billing::INVALID_WEBHOOK_SIGNATURE, "error": data.message})),
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
            Json(json!({"error_code": crate::errors::TOOL_NOT_FOUND, "error": "no published server.json for this namespace"})),
        )
            .into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error_code": crate::errors::STORAGE, "error": "storage error"})),
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
            Json(json!({"error_code": crate::errors::STORAGE, "error": "storage error"})),
        )
            .into_response(),
    }
}

/// PRD-mcphost-oauth-first-entry-url requirement 3 (AC2): the entry
/// document, served at `oauth::entry_metadata_path()`.
async fn well_known_oauth_protected_resource_entry(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(crate::oauth::protected_resource_metadata_for_entry(&state))
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
                Json(json!({"error_code": crate::errors::TENANT_NOT_FOUND, "error": "no such tenant"})),
            )
                .into_response();
        }
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error_code": crate::errors::STORAGE, "error": "storage error"})),
            )
                .into_response();
        }
    };
    match crate::authz::authorization_server_metadata_for_tenant(&state, &tenant).await {
        Ok(doc) => (StatusCode::OK, Json(doc)).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error_code": crate::errors::STORAGE, "error": "storage error"})),
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
                Json(json!({"error_code": crate::errors::TENANT_NOT_FOUND, "error": "no such tenant"})),
            )
                .into_response();
        }
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error_code": crate::errors::STORAGE, "error": "storage error"})),
            )
                .into_response();
        }
    };
    match crate::oauth::protected_resource_metadata_for_tenant(&state, &tenant).await {
        Ok(doc) => (StatusCode::OK, Json(doc)).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error_code": crate::errors::STORAGE, "error": "storage error"})),
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
            Json(json!({"error_code": crate::errors::STORAGE, "error": "storage error"})),
        )
            .into_response(),
    }
}

/// PRD-mcphost-server-capabilities-from-registry requirement 3: `GET
/// /capabilities.json`, unauthenticated and DB-free (so safe keyless under
/// the signup rate limit), rendered from the same `CAPABILITIES` const as
/// `initialize.capabilities`. `?implemented=1` keeps only implemented rows.
async fn capabilities_json(Query(params): Query<HashMap<String, String>>) -> Response {
    let only_implemented = params.get("implemented").is_some_and(|v| v == "1");
    (
        StatusCode::OK,
        [(CACHE_CONTROL, HeaderValue::from_static("max-age=60"))],
        Json(crate::capabilities::capabilities_document(only_implemented)),
    )
        .into_response()
}

/// PRD-mcphost-sse-frame-contract-generated requirement 3: `GET
/// /protocol/frames.json`, the frames each request type may carry, rendered
/// from `FRAME_SEQUENCES` by `scripts/protocol-frames-check.sh` and embedded
/// at compile time like the other `www/` files.
async fn protocol_frames_json() -> Response {
    (
        StatusCode::OK,
        [
            (CONTENT_TYPE, HeaderValue::from_static("application/json")),
            (CACHE_CONTROL, HeaderValue::from_static("max-age=300")),
        ],
        include_str!("../www/frames.json"),
    )
        .into_response()
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

/// Twin of [`static_text_page`] for the generated `www/retry_policy.json`.
fn static_json_page(content: &'static str) -> Response {
    (
        StatusCode::OK,
        [("content-type", "application/json")],
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

/// `GET /llms-install.md` (PRD-mcphost-llms-install-doc): the install
/// document an agent reads to configure its own client, rendered from the
/// install table over this server's public URL -- the same bytes the repo
/// root `llms-install.md` commits (`mcphost gen-docs`). No auth; cacheable
/// for ten minutes. P1 requirement 4 (AC6): each fetch records an
/// `install_doc` funnel event, `origin` by IP class
/// ([`crate::state::classify_funnel_origin`]) -- same table and origin
/// vocabulary as `connect_go`'s `install_link:<client>` events.
async fn llms_install_doc(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Response {
    let ip = crate::claim::source_ip(&headers, peer);
    let origin = crate::state::classify_funnel_origin(&ip, false, &state.fleet_ips);
    if let Err(e) = state.db.record_oauth_funnel_event("install_doc", origin).await {
        tracing::warn!(error = %e, "failed to record install_doc funnel event");
    }
    (
        StatusCode::OK,
        [
            ("content-type", "text/markdown; charset=utf-8"),
            ("cache-control", "public, max-age=600"),
        ],
        crate::install_links::render_install_doc(&state.public_url),
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
        None => match crate::error_registry::lookup(&code) {
            Some(e) => {
                crate::help::record_help_served(e.code);
                let support_url = std::env::var("MCPHOST_SUPPORT_URL").ok();
                crate::claim::html_response(
                    StatusCode::OK,
                    crate::help::render_registry_help_page(e, support_url.as_deref()),
                )
            }
            None => crate::claim::html_response(
                StatusCode::NOT_FOUND,
                "no help page for that code".to_string(),
            ),
        },
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

/// `GET /events.json` (PRD-mcphost-event-kinds-registry-generated R3): the
/// event-kind registry rendered by `events::events_json`.
async fn events_json_route() -> Response {
    (
        StatusCode::OK,
        [(CACHE_CONTROL, HeaderValue::from_static("public, max-age=300"))],
        Json(crate::events::events_json()),
    )
        .into_response()
}

/// `GET /errors.json` (PRD-mcphost-errors-json-contract R3, AC3): every error
/// code the host can return, generated from
/// [`crate::error_registry::ERROR_CODES`] -- the same object `host.errors`
/// returns. Anonymous; cacheable for five minutes (it only changes with a
/// release).
async fn errors_json_route(State(state): State<Arc<AppState>>) -> Response {
    (
        StatusCode::OK,
        [("cache-control", "public, max-age=300")],
        Json(crate::error_registry::catalog_json(&state.public_url)),
    )
        .into_response()
}

/// PRD-mcphost-contract-version-reported R5: `GET /contract.json`,
/// anonymous. The body is pre-serialized at boot; `ETag` is the quoted
/// contract sha and a matching `If-None-Match` gets an empty 304.
async fn contract_json_route(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let etag = format!("\"{}\"", state.contract.sha);
    let matches = headers
        .get(axum::http::header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(',')
                .map(|t| t.trim().trim_start_matches("W/"))
                .any(|t| t == etag || t == "*")
        });
    let mut response = if matches {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        let mut r = Response::new(Body::from(state.contract.served.clone()));
        r.headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        r
    };
    if let Ok(value) = HeaderValue::from_str(&etag) {
        response.headers_mut().insert(axum::http::header::ETAG, value);
    }
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("max-age=300"));
    response
}

/// PRD-mcphost-heal-action-registry-and-ledger R4: `GET /incidents.json`,
/// anonymous, last 24 h of action rows through the allowlist anonymiser.
async fn incidents_json_route(State(state): State<Arc<AppState>>) -> Response {
    let mut response = match crate::heal::incidents_json(&state).await {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error_code": crate::errors::STORAGE, "error": "storage error"})),
        )
            .into_response(),
    };
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("max-age=30"));
    response
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
            Json(json!({"error_code": crate::errors::STORAGE, "error": "storage error"})),
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
pub async fn protocol_version_and_log(req: Request<Body>, next: Next) -> Response {
    let start = Instant::now();
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let mcp_name = req
        .headers()
        .get("Mcp-Name")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    // PRD-mcphost-incident-banner-for-agents R7: whoever attaches a banner
    // to this request's response records its id in this cell.
    let attached = std::cell::Cell::new(None);
    let (mut response, attached) = crate::incident_banner::ATTACHED
        .scope(attached, async {
            let response = next.run(req).await;
            (response, crate::incident_banner::ATTACHED.with(std::cell::Cell::get))
        })
        .await;

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
        incident = attached,
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
/// 403 with the RFC 6750 §3 `insufficient_scope` error challenge. Every
/// 401/403 challenge this middleware writes now also carries `scope="mcp"`
/// (spec 2026-07-28 requirement 4). A no-op for every other route
/// (`signup`/`host.quickstart`/`billing.plans`/`host.redeem` never reach
/// `tenant_key_missing` in the first place -- see `handler::call_tool`'s
/// own match-arm ordering -- so this never fires for them).
async fn oauth_401_upgrade(State(state): State<Arc<AppState>>, req: Request<Body>, next: Next) -> Response {
    let path = req.uri().path();
    let metadata_url = if path == "/mcp" {
        format!("{}/.well-known/oauth-protected-resource", state.public_url)
    } else if path == crate::oauth::OAUTH_ENTRY_PATH {
        crate::oauth::entry_metadata_url(&state.public_url)
    } else if let Some(ns) = path.strip_prefix("/t/").and_then(|s| s.strip_suffix("/mcp")) {
        crate::oauth::tenant_metadata_url(&state.public_url, ns)
    } else {
        return next.run(req).await;
    };
    let response = next.run(req).await;
    // PRD-mcphost-elicitation-in-band-on-stateless-transport R0: only a JSON
    // body can carry a JSON-RPC auth refusal; an SSE `tools/call` (an
    // in-band elicitation) must stream, not be buffered until it closes.
    let is_json = response
        .headers()
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("application/json"));
    if !is_json {
        return response;
    }
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
        Some(crate::scopes::INSUFFICIENT_SCOPE) => {
            *response.status_mut() = StatusCode::FORBIDDEN;
            if let Ok(value) = HeaderValue::from_str(&format!(
                "Bearer error=\"{code}\", scope=\"{scope}\", resource_metadata=\"{metadata_url}\"",
                code = crate::scopes::INSUFFICIENT_SCOPE
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
         <p>No key is needed on /mcp either way: an agent that connects there directly and \
         makes its own first call gets this same URL back as <code>onboarding.url</code> in \
         the response, with no signup call of its own.</p>\
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
fn render_new_url_result_page(url: &str, claim_url: Option<&str>) -> String {
    let url = crate::claim::html_escape(url);
    // PRD-mcphost-ownership-moment requirement 1 (AC1): this mint's own
    // `onboarding` envelope -- `/u/new` is an HTML page, not a tool
    // response, so the claim link rides the page itself rather than a JSON
    // field. Best-effort (`Option`): the page still works if, somehow, no
    // claim token was minted.
    let claim_html = claim_url
        .map(|u| {
            let u = crate::claim::html_escape(u);
            format!(
                "<p>Own this tenant: open <code>{u}</code> and verify your email to claim it.</p>"
            )
        })
        .unwrap_or_default();
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
             <p>Generic JSON:</p><p><code>{{&quot;url&quot;: &quot;{url}&quot;}}</code></p>\
             {claim_html}",
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
    let (tenant_id, claim_url) = match signup_result {
        Ok(value) => {
            let Some(namespace) = value.get("tenant").and_then(Value::as_str) else {
                return crate::claim::html_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    render_new_url_storage_error(),
                );
            };
            let claim_url = value.get("claim_url").and_then(Value::as_str).map(str::to_string);
            match state.db.find_tenant_by_namespace(namespace.to_string()).await {
                Ok(Some(tenant)) => (tenant.id, claim_url),
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
    crate::claim::html_response(
        StatusCode::OK,
        render_new_url_result_page(&url, claim_url.as_deref()),
    )
}

/// PRD-mcphost-client-install-links P0 requirement 2 (AC2): `GET /connect`
/// -- plain HTML, no JavaScript, reachable with no `Authorization` header
/// at all. Renders the four [`crate::install_links::for_url`] forms over
/// this host's own `state.public_url` by default.
///
/// P0 requirement 2's "the personal-URL variant renders when the request
/// carries a valid tenant": the only way this host can show a REAL
/// personal URL (never stored in plaintext -- only its hash is, see
/// `require_known_url_secret`'s own doc comment) is for the caller to
/// already possess it. `?u=<personal mcp url>` (P2 requirement 8's own
/// mechanism: "a `?u=<personal url>` query renders personal links for a
/// logged-out second device, link carried by the claim email") covers
/// exactly that case with no DB lookup and no auth at all -- the caller
/// already has the secret, same trust boundary as pasting it into a
/// client directly. See [`personal_base_from_query`] for why this can
/// never become an open-redirect/arbitrary-link surface.
#[derive(serde::Deserialize)]
struct ConnectQuery {
    u: Option<String>,
}

async fn connect_page(State(state): State<Arc<AppState>>, Query(params): Query<ConnectQuery>) -> Response {
    let base = personal_base_from_query(&state, params.u.as_deref())
        .unwrap_or_else(|| state.public_url.clone());
    let links = crate::install_links::for_url(&base);
    crate::claim::html_response(StatusCode::OK, render_connect_page(&links))
}

/// `u` must be EXACTLY this host's own `{public_url}/u/{secret}/mcp` shape
/// (secret: non-empty, ASCII alphanumeric only -- the same alphabet
/// [`crate::auth::generate_url_secret`] actually draws from); anything
/// else (a foreign URL, a path-shaped string with extra segments, a
/// secret carrying HTML-significant characters) is ignored, falling back
/// to the anonymous endpoint, rather than ever being reflected into the
/// page verbatim.
fn personal_base_from_query(state: &AppState, u: Option<&str>) -> Option<String> {
    let prefix = format!("{}/u/", state.public_url.trim_end_matches('/'));
    let secret = u?.strip_prefix(&prefix)?.strip_suffix("/mcp")?;
    if secret.is_empty() || !secret.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    Some(format!("{prefix}{secret}"))
}

/// AC2: every one of [`crate::install_links::Links`]'s four forms, plus
/// the plain `mcp_url` text (Migration note: "clients without deep-link
/// support still get the plain URL on the page").
///
/// P1 requirement 5 (AC5): the Cursor/VS Code links point at
/// `/connect/go/{client}`, not the deep link directly -- that route
/// records an `install_link` funnel event and 302s on to the real deep
/// link (see [`connect_go`]'s own doc comment); Claude Code/Claude.ai have
/// no deep link of their own, so their blocks stay plain `<code>` text,
/// no link to route through.
fn render_connect_page(links: &crate::install_links::Links) -> String {
    use crate::install_links::Kind;
    let esc = |s: &str| crate::claim::html_escape(s);
    let mut body = format!(
        "<h1>Connect mcphost</h1>\
         <p>Pick your client -- one click, or one pasted command, and the server is \
         connected. No JavaScript runs on this page; every link below is a plain, \
         navigable URL, and the raw endpoint always works too: <code>{}</code></p>",
        esc(&links.mcp_url)
    );
    for kind in Kind::ALL {
        body.push_str(&format!("<h2>{}</h2>", kind.heading()));
        for s in links.iter().filter(|s| s.kind == kind) {
            body.push_str(&format!("<h3 id=\"{}\">{}</h3>", s.id, esc(s.label)));
            match kind {
                Kind::Command => body.push_str(&format!("<p>Run: <code>{}</code></p>", esc(&s.artefact))),
                Kind::Deeplink => body.push_str(&format!(
                    "<p><a href=\"/connect/go/{id}\">Add to {label}</a> -- or add <code>{url}</code> \
                     to your MCP config directly.</p>",
                    id = s.id,
                    label = esc(s.label),
                    url = esc(&links.mcp_url),
                )),
                Kind::Json => body.push_str(&format!("<pre><code>{}</code></pre>", esc(&s.artefact))),
                Kind::Steps => {
                    body.push_str("<ol>");
                    for line in s.artefact.lines() {
                        let text = line.split_once(". ").map_or(line, |(_, rest)| rest);
                        body.push_str(&format!("<li>{}</li>", esc(text)));
                    }
                    body.push_str("</ol>");
                }
            }
            body.push_str(&format!(
                "<p><small>Docs: <a href=\"{url}\">{url}</a> (checked {checked})</small></p>",
                url = esc(s.doc_url),
                checked = esc(s.checked),
            ));
        }
    }
    crate::claim::page("mcphost — connect", &body)
}

/// `GET /connect/go/{client}` (PRD-mcphost-client-install-links P1
/// requirement 5, AC5): records an `install_link` funnel event --
/// `event: "install_link:<client>"`, reusing `oauth_funnel_events` rather
/// than a new table (Migration/compatibility: "no schema change beyond a
/// new funnel event kind") with [`crate::state::classify_funnel_origin`]'s
/// verdict for this request's own source IP -- then 302s to the client's
/// deep link (`deeplink` kind) or renders the surface's instruction page
/// (every other kind). Any `client` outside the install table's twelve ids
/// is 404 and records nothing.
#[derive(serde::Deserialize)]
struct ConnectGoQuery {
    format: Option<String>,
}

async fn connect_go(
    State(state): State<Arc<AppState>>,
    Path(client): Path<String>,
    Query(params): Query<ConnectGoQuery>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Response {
    // Legacy hyphenated ids (`claude-code`, `claude-ai`) still resolve.
    let id = client.replace('-', "_");
    let Some(surface) = crate::install_links::surface_for(&state.public_url, &id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    // PRD-mcphost-quickstart-by-client R5: `?format=json` is the same
    // `client` block `host.quickstart` returns (`from` is `"path"`: the id
    // came from the URL). Docs fetch it; it is not an install click, so no
    // funnel event.
    if params.format.as_deref() == Some("json") {
        let block = crate::install_links::client_block(&state.public_url, Some((surface.id, "path")));
        return (StatusCode::OK, axum::Json(block)).into_response();
    }
    let ip = crate::claim::source_ip(&headers, peer);
    let origin = crate::state::classify_funnel_origin(&ip, false, &state.fleet_ips);
    let event = format!("install_link:{}", surface.id);
    if let Err(e) = state.db.record_oauth_funnel_event(&event, origin).await {
        tracing::warn!(error = %e, client = %client, "failed to record install_link funnel event");
    }
    if surface.kind != crate::install_links::Kind::Deeplink {
        return crate::claim::html_response(StatusCode::OK, render_instruction_page(&surface));
    }
    let target = surface.artefact;
    let Ok(location) = HeaderValue::from_str(&target) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    match Response::builder().status(StatusCode::FOUND).header(LOCATION, location).body(Body::empty()) {
        Ok(resp) => resp,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// `/connect/go/{id}` for a surface with no link of its own (command, JSON,
/// steps): the same artefact `/connect` shows, on a page of its own.
fn render_instruction_page(s: &crate::install_links::Surface) -> String {
    let esc = |t: &str| crate::claim::html_escape(t);
    let body = match s.kind {
        crate::install_links::Kind::Steps => {
            let items: String = s
                .artefact
                .lines()
                .map(|l| format!("<li>{}</li>", esc(l.split_once(". ").map_or(l, |(_, r)| r))))
                .collect();
            format!("<ol>{items}</ol>")
        }
        _ => format!("<pre><code>{}</code></pre>", esc(&s.artefact)),
    };
    crate::claim::page(
        &format!("mcphost — connect {}", s.label),
        &format!(
            "<h1>Connect {label}</h1>{body}<p><small>Docs: <a href=\"{url}\">{url}</a> (checked {checked})</small></p>\
             <p><a href=\"/connect\">All clients</a></p>",
            label = esc(s.label),
            url = esc(s.doc_url),
            checked = esc(s.checked),
        ),
    )
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
    match state.db.find_tenant_by_url_secret_hash(hash.clone()).await {
        Ok(Some(_)) => {}
        // PRD-mcphost-same-user-second-agent R2/R4: a sibling client's
        // secret (live, revoked, or rotated away from) reaches dispatch,
        // where a dead one answers `tenant_key_invalid`.
        Ok(None) => {
            let known = state.db.find_client_by_secret_hash(hash.clone()).await.is_ok_and(|c| c.is_some())
                || state.db.is_retired_client_secret(hash).await.unwrap_or(false);
            if !known {
                return StatusCode::NOT_FOUND.into_response();
            }
        }
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
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

/// PRD-mcphost-same-user-second-agent R3: `/j/{code}/mcp` for a code that
/// is unknown, revoked or expired is 404 before the MCP service sees the
/// request (an exhausted code reaches dispatch, where the call that loses
/// the race answers `join_code_invalid` with its `next_call`). A browser's
/// `GET` on a live code gets a short explainer that names no tenant.
async fn require_live_join_code(
    State(state): State<Arc<AppState>>,
    Path(code): Path<String>,
    req: Request<Body>,
    next: Next,
) -> Response {
    if !state.db.join_code_connectable(crate::auth::hash_key(&code)).await.unwrap_or(false) {
        return crate::claim::html_response(
            StatusCode::NOT_FOUND,
            crate::claim::page(
                "mcphost — join link not valid",
                "<h1>This join link is no longer valid</h1>\
                 <p>It may have expired or been replaced. Ask your other client to call \
                 <code>host.whoami</code> for the current <code>join_url</code>.</p>",
            ),
        );
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
                "mcphost — join your workspace",
                "<h1>Join your mcphost workspace</h1>\
                 <p>Add this URL as an MCP server in your other client and ask it to call any \
                 tool -- that first call gives it its own URL in the same workspace, no key and \
                 no email.</p>",
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


/// PRD-mcphost-oauth-first-entry-url requirement 2 (AC1): every request on
/// the OAuth-first entry path answers 401 + `WWW-Authenticate: Bearer
/// resource_metadata="<entry document>", scope="mcp"` unless it carries a
/// bearer, BEFORE any JSON-RPC parsing and for every method including
/// `initialize` -- the one thing an OAuth-only client (Claude.ai, ChatGPT)
/// needs to start discovery. Runs outside (and in front of) the implicit-
/// signup dispatch, so a credential-less request never creates a tenant.
async fn require_entry_bearer(State(state): State<Arc<AppState>>, req: Request<Body>, next: Next) -> Response {
    let Some(token) = crate::auth::extract_bearer(req.headers()) else {
        return entry_unauthorized(&state, None);
    };
    // requirement 4: only a valid OAuth token minted for the entry resource
    // gets past the gate -- a raw tenant key, a malformed bearer, or a token
    // for another audience is 401 `invalid_token` / `wrong_audience`.
    if token.split('.').count() != 3 {
        return entry_unauthorized(&state, Some("malformed"));
    }
    let verdict = match crate::oauth::validate_bearer(&state, &token).await {
        Ok(caller) => crate::oauth::check_entry_audience(&state.public_url, req.uri().path(), &caller),
        Err(e) => Err(e),
    };
    match verdict {
        Ok(()) => next.run(req).await,
        Err(crate::errors::AppError::InvalidToken(reason)) => entry_unauthorized(&state, Some(reason)),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error_code": crate::errors::STORAGE, "error": "storage error"})),
        )
            .into_response(),
    }
}

/// The entry path's 401: the challenge names the entry document; the body
/// is the JSON-RPC error shape every other 401 on this server carries
/// (`error.data.error_code` / `error_description`).
fn entry_unauthorized(state: &AppState, invalid: Option<&'static str>) -> Response {
    let metadata_url = crate::oauth::entry_metadata_url(&state.public_url);
    let data = match invalid {
        Some(reason) => json!({"error_code": crate::errors::INVALID_TOKEN, "error_description": reason}),
        None => json!({
            "error_code": AUTHORIZATION_REQUIRED,
            "error": "this URL is OAuth-only: authorize through the metadata named in WWW-Authenticate",
        }),
    };
    let body = json!({
        "jsonrpc": "2.0",
        "id": null,
        "error": {"code": -32001, "message": "authorization required", "data": data},
    });
    let mut response = (StatusCode::UNAUTHORIZED, Json(body)).into_response();
    if let Ok(value) = HeaderValue::from_str(&format!("Bearer resource_metadata=\"{metadata_url}\", scope=\"mcp\"")) {
        response.headers_mut().insert(axum::http::header::WWW_AUTHENTICATE, value);
    }
    response
}

/// The protocol revision a message speaks: the `MCP-Protocol-Version` header,
/// else the request's `_meta` protocol version, else `None`.
fn message_version<'a>(msg: &'a Value, header: Option<&'a str>) -> Option<&'a str> {
    header.or_else(|| msg.pointer("/params/_meta")?.get(crate::capabilities::META_PROTOCOL_VERSION_KEY)?.as_str())
}

/// PRD-mcphost-capabilities-generated-from-dispatch R1: the one pre-dispatch
/// refusal site. Reads the JSON-RPC `method` of a POST body (each element of a
/// batch) and, when [`crate::capabilities::refusal`] names a refusal for it,
/// answers that `-32601` itself, before `rmcp` parses the params -- so the
/// labelled reply never depends on a header only some clients send. Anything
/// else (no body, not JSON, no `method`, a notification, a mixed batch) is
/// passed through with its body restored.
async fn refuse_unimplemented_methods(
    State((state, enabled)): State<(Arc<AppState>, bool)>,
    req: Request<Body>,
    next: Next,
) -> Response {
    if !enabled || req.method() != Method::POST {
        return next.run(req).await;
    }
    let (parts, body) = req.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, MAX_REQUEST_BODY_BYTES).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    if let Ok(doc) = serde_json::from_slice::<Value>(&bytes) {
        let header_version = parts
            .headers
            .get("mcp-protocol-version")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let refuse = |msg: &Value| -> Option<Value> {
            let method = msg.get("method")?.as_str()?;
            let id = msg.get("id")?.clone();
            let err = crate::capabilities::gate_error(method, message_version(msg, header_version.as_deref()), Some(&state.public_url))?;
            Some(json!({"jsonrpc": "2.0", "id": id, "error": err}))
        };
        let reply = match &doc {
            Value::Array(items) if !items.is_empty() => {
                items.iter().map(refuse).collect::<Option<Vec<Value>>>().map(Value::Array)
            }
            single => refuse(single),
        };
        if let Some(reply) = reply {
            let first = doc.as_array().and_then(|a| a.first()).unwrap_or(&doc);
            return (crate::capabilities::refusal_status(message_version(first, header_version.as_deref())), Json(reply)).into_response();
        }
    }
    next.run(Request::from_parts(parts, Body::from(bytes))).await
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
    // PRD-mcphost-delegation-bound-to-run-or-session R5: the client ending its
    // session (DELETE, with the id this process minted) kills every key
    // bound to it, immediately.
    if req.method() == http::Method::DELETE
        && let Some(ended) = &presented
        && let Err(e) = state.db.revoke_session_bound(ended.clone(), crate::state::now_unix()).await
    {
        tracing::warn!(error = %e, "failed to revoke session-bound keys on session close");
    }
    if req.method() == http::Method::DELETE
        && let Some(ended) = &presented
    {
        state.session_bindings.inband().forget(ended);
    }
    let session_present = presented.is_some();
    let session_id = presented.unwrap_or_else(|| state.session_bindings.issue());
    req.extensions_mut()
        .insert(crate::session_bind::RequestSessionId(session_id.clone()));
    req.extensions_mut().insert(crate::session_bind::SessionPresented(session_present));
    let mut response = next.run(req).await;
    if let Ok(value) = HeaderValue::from_str(&session_id) {
        response.headers_mut().insert(header, value);
    }
    response
}

/// PRD-mcphost-elicitation-in-band-on-stateless-transport R3: rmcp's
/// stateless transport answers a POSTed client JSON-RPC *response* `202` and
/// drops it, so the answer to an in-band `elicitation/create` /
/// `sampling/createMessage` is taken here instead and handed to the waiting
/// call, keyed by (session id, request id). Runs inside
/// [`issue_session_id`], so the session id is the verified one. Only a
/// session with a request in flight has its body read; every other request
/// -- and every body that is not a response -- is forwarded unchanged. An
/// answer whose id is unknown or already completed is answered `202` and
/// delivered nowhere.
async fn deliver_client_response(State(state): State<Arc<AppState>>, req: Request<Body>, next: Next) -> Response {
    let session = req
        .extensions()
        .get::<crate::session_bind::RequestSessionId>()
        .map(|s| s.0.clone());
    let stores = state.session_bindings.inband();
    let Some(session) = session.filter(|s| req.method() == Method::POST && stores.has_inflight(s)) else {
        return next.run(req).await;
    };
    let (parts, body) = req.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, MAX_REQUEST_BODY_BYTES).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    if let Some((id, answer)) = crate::inband::client_response(&bytes) {
        stores.deliver(&session, &id, answer);
        return StatusCode::ACCEPTED.into_response();
    }
    next.run(Request::from_parts(parts, Body::from(bytes))).await
}

/// PRD-mcphost-reachability-alt-host requirement 4 / AC4: `Strict-
/// Transport-Security` on every response this binary serves, when
/// `MCPHOST_PUBLIC_URL` itself is configured `https://` -- gated on the
/// scheme (not unconditional) because a loopback dev server or test
/// harness running on a bare `http://127.0.0.1:PORT` `public_url` must
/// never tell a browser to upgrade an address it doesn't actually serve
/// over TLS. `max-age=15552000` (180 days) meets the AC's `>= 15552000`
/// floor.
const HSTS_HEADER_VALUE: &str = "max-age=15552000; includeSubDomains";

async fn hsts(State(state): State<Arc<AppState>>, req: Request<Body>, next: Next) -> Response {
    let mut response = next.run(req).await;
    if state.public_url.starts_with("https://") {
        response
            .headers_mut()
            .insert(STRICT_TRANSPORT_SECURITY, HeaderValue::from_static(HSTS_HEADER_VALUE));
    }
    response
}

pub fn build_router(state: Arc<AppState>) -> Router {
    build_router_with_session_mode(state, crate::inband::PROD_LEGACY_SESSION_MODE)
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
    build_router_inner(state, legacy_session_mode, true)
}

/// Test-only seam (PRD-mcphost-capabilities-generated-from-dispatch AC1): the
/// production router minus the pre-dispatch gate, so the premise -- a bare
/// `-32601` for `roots/list {}` -- is reproduced against the same tree.
pub fn build_router_without_method_gate(state: Arc<AppState>) -> Router {
    build_router_inner(state, false, false)
}

fn build_router_inner(state: Arc<AppState>, legacy_session_mode: bool, method_gate: bool) -> Router {
    let config = StreamableHttpServerConfig::default()
        .with_json_response(crate::inband::PROD_JSON_RESPONSE)
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
        .route_layer(middleware::from_fn_with_state((state.clone(), method_gate), refuse_unimplemented_methods))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_known_tenant_namespace,
        ))
        // PRD-mcphost-session-bound-tenant-after-signup requirement 1: a
        // `/t/{ns}/mcp` connection gets a session identity on exactly the
        // same terms as `/mcp` -- see `root_mcp_router` below.
        .route_layer(middleware::from_fn_with_state(state.clone(), deliver_client_response))
        .route_layer(middleware::from_fn_with_state(state.clone(), issue_session_id));

    // PRD-mcphost-url-bound-tenants requirement 1: `/u/{secret}/mcp` serves
    // the identical streamable-HTTP service as `/mcp`/`/t/{ns}/mcp` --
    // path-secret resolution happens inside `handler.rs`
    // (`resolve_path_secret_auth`), not here. Same `route_layer` shape as
    // `tenant_mcp_router` above, for the same "scoped to the route actually
    // matched" reason.
    let url_mcp_router = Router::new()
        .route_service("/u/{secret}/mcp", service.clone())
        .route_layer(middleware::from_fn_with_state((state.clone(), method_gate), refuse_unimplemented_methods))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_known_url_secret,
        ))
        .route_layer(middleware::from_fn_with_state(state.clone(), deliver_client_response))
        .route_layer(middleware::from_fn_with_state(state.clone(), issue_session_id));

    // PRD-mcphost-invite-links requirement 2: `/i/{code}/mcp` serves the
    // identical streamable-HTTP service as `/mcp`/`/u/{secret}/mcp` --
    // credential/tenant-creation resolution happens inside `handler.rs`
    // (`handler::call_tool`'s own invite-claim step), not here. Same
    // `route_layer` shape as `url_mcp_router` above.
    let invite_mcp_router = Router::new()
        .route_service("/i/{code}/mcp", service.clone())
        .route_layer(middleware::from_fn_with_state((state.clone(), method_gate), refuse_unimplemented_methods))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_live_invite_code,
        ))
        .route_layer(middleware::from_fn_with_state(state.clone(), deliver_client_response))
        .route_layer(middleware::from_fn_with_state(state.clone(), issue_session_id));

    // PRD-mcphost-org-model R4: `/o/{code}/mcp` is the org flavor of the
    // invite path: same service, same implicit-signup claim, and the claim
    // adds the membership row (`kind = 'org'` on the invite).
    let org_mcp_router = Router::new()
        .route_service("/o/{code}/mcp", service.clone())
        .route_layer(middleware::from_fn_with_state((state.clone(), method_gate), refuse_unimplemented_methods))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_live_invite_code,
        ))
        .route_layer(middleware::from_fn_with_state(state.clone(), deliver_client_response))
        .route_layer(middleware::from_fn_with_state(state.clone(), issue_session_id));

    // PRD-mcphost-same-user-second-agent R3: `/j/{code}/mcp` serves the
    // identical service; the first `host.*` call on it mints a sibling
    // client of the code's tenant (`handler::call_tool`'s join step).
    let join_mcp_router = Router::new()
        .route_service("/j/{code}/mcp", service.clone())
        .route_layer(middleware::from_fn_with_state((state.clone(), method_gate), refuse_unimplemented_methods))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_live_join_code))
        .route_layer(middleware::from_fn_with_state(state.clone(), deliver_client_response))
        .route_layer(middleware::from_fn_with_state(state.clone(), issue_session_id));

    // PRD-mcphost-session-bound-tenant-after-signup requirement 1: only the
    // streamable-HTTP routes get a session identity, so `/healthz`, the
    // OAuth endpoints and the router's own 404 fallback are untouched. Its
    // own sub-router with `route_layer` (rather than `.layer()` on the
    // merged router) for exactly the reason `tenant_mcp_router` above
    // already documents: `.layer()` would wrap the default fallback too.
    let root_mcp_router = Router::new()
        .route_service("/mcp", service.clone())
        .route_layer(middleware::from_fn_with_state((state.clone(), method_gate), refuse_unimplemented_methods))
        .route_layer(middleware::from_fn_with_state(state.clone(), deliver_client_response))
        .route_layer(middleware::from_fn_with_state(state.clone(), issue_session_id));

    // PRD-mcphost-oauth-first-entry-url requirement 2: `OAUTH_ENTRY_PATH`
    // serves the identical streamable-HTTP service as `/mcp`, behind a gate
    // that 401s (with the entry document's challenge) before the service
    // ever parses a body. Same `route_layer` shape as the routers above.
    let entry_mcp_router = Router::new()
        .route_service(crate::oauth::OAUTH_ENTRY_PATH, service.clone())
        .route_layer(middleware::from_fn_with_state((state.clone(), method_gate), refuse_unimplemented_methods))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_entry_bearer))
        .route_layer(middleware::from_fn_with_state(state.clone(), deliver_client_response))
        .route_layer(middleware::from_fn_with_state(state.clone(), issue_session_id));

    crate::statusfeed::mark_process_start();
    // R1: every spec'd route is registered from `routes::ROUTES` (handlers
    // included), so a route cannot be served without a spec entry. Routes
    // sharing a path (one entry per method) are merged first. A `cors` path
    // gets its own sub-router with `route_layer` (not `.layer()`), so
    // `add_cors_header` (PRD-mcphost-public-tool-url requirement 11, AC10)
    // never wraps the router's default-NotFound fallback or any other route.
    type PathEntry = (&'static str, bool, axum::routing::MethodRouter<Arc<AppState>>);
    let mut by_path: Vec<PathEntry> = Vec::new();
    for spec in routes::ROUTES {
        let mounted = (spec.mount)();
        match by_path.iter_mut().find(|(path, _, _)| *path == spec.path) {
            Some((_, _, merged)) => *merged = std::mem::take(merged).merge(mounted),
            None => by_path.push((spec.path, spec.cors, mounted)),
        }
    }
    let mut table_router: Router<Arc<AppState>> = Router::new();
    for (path, cors, method_router) in by_path {
        let one = Router::new().route(path, method_router);
        table_router = table_router.merge(if cors {
            one.route_layer(middleware::from_fn(crate::public_tool::add_cors_header))
        } else {
            one
        });
    }

    crate::statusfeed::mark_process_start();
    Router::new()
        .merge(table_router)
        // mcphost-polish-p0-20260930 (audit finding 1), generalized by
        // PRD-mcphost-first-hour-support-surface: see `static_page`'s own
        // doc comment -- same compile-time-embedded `www/*.html` pattern,
        // one shared helper instead of one named `async fn` per page.
        // These static pages are `routes::UNSPECCED`.
        .route("/status.html", get(|| async { static_page(include_str!("../www/status.html")) }))
        .route("/aup.html", get(|| async { static_page(include_str!("../www/aup.html")) }))
        // requirement 3 (AC4): a static shell that fetches the live
        // `/plans.json` client-side -- see `www/plans.html`'s own comment.
        .route("/plans.html", get(|| async { static_page(include_str!("../www/plans.html")) }))
        // requirement 5/8 (AC6, AC8): README/llms.txt already link
        // `/llms.txt` (absolute, as `https://mcphost.dev/llms.txt`) and
        // `/llms-full.txt` -- routed here for the same reason status/aup
        // are (requirement 8: every `www/` page is served *by this
        // binary*, not left to a reverse proxy's own static-file config to
        // get right or drift on).
        .route("/llms.txt", get(|| async { static_text_page(include_str!("../www/llms.txt")) }))
        .route("/llms-full.txt", get(|| async { static_text_page(include_str!("../www/llms-full.txt")) }))
        // PRD-mcphost-retry-policy-table-generated R4: the machine-readable
        // `RETRY_POLICY`, generated by `mcphost retry-policy`.
        .route("/retry_policy.json", get(|| async { static_json_page(include_str!("../www/retry_policy.json")) }))
        // PRD-mcphost-www-trust-pages requirement 4 / support_ac06: this
        // binary serves `/skill.md` itself, same as `/llms.txt` above --
        // prod's Caddy route was the only thing serving it (404'd from
        // here, the same gap status.html/aup.html's own hotfix closed).
        .route("/skill.md", get(|| async { static_markdown_page(include_str!("../www/skill.md")) }))
        .route("/llms-install.md", get(llms_install_doc))
        // PRD-mcphost-llms-install-doc requirement 5: the sitemap lists
        // `/llms-install.md`; served here so the file is not only a deploy
        // artefact.
        .route(
            "/sitemap.xml",
            get(|| async {
                (
                    StatusCode::OK,
                    [("content-type", "application/xml; charset=utf-8")],
                    include_str!("../www/sitemap.xml"),
                )
                    .into_response()
            }),
        )
        .route("/support.html", get(support_page))
        .merge(root_mcp_router)
        .merge(tenant_mcp_router)
        .merge(url_mcp_router)
        .merge(invite_mcp_router)
        .merge(org_mcp_router)
        .merge(join_mcp_router)
        .merge(entry_mcp_router)
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
        // PRD-mcphost-reachability-alt-host requirement 4: added LAST, so
        // (same "last `.layer()` call becomes the outermost middleware"
        // rule the comment above already spells out for
        // `oauth_401_upgrade`/`protocol_version_and_log`) it runs on
        // literally every response this router produces, including the
        // default 404 fallback.
        .layer(middleware::from_fn_with_state(state.clone(), hsts))
        // PRD-mcphost-graceful-drain-contract R1: outermost, so a draining
        // process refuses a new `/mcp` POST on a still-open keep-alive
        // connection the same way the closed listener refuses a new socket.
        .layer(middleware::from_fn(drain_gate))
        .with_state(state)
}

/// 503 for a new `/mcp` POST (any of the MCP route families) once the drain
/// has begun; every other request passes.
async fn drain_gate(req: Request<Body>, next: Next) -> Response {
    if let Some(restart_id) = crate::drain::global().restart_id()
        && req.method() == Method::POST
        && req.uri().path().ends_with("/mcp")
    {
        // Read the body out first: closing a connection on an unread request
        // body resets it, and the client would see a reset, not the 503.
        let _ = axum::body::to_bytes(req.into_body(), MAX_REQUEST_BODY_BYTES).await;
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"state": "draining", "restart_id": restart_id})),
        )
            .into_response();
    }
    next.run(req).await
}

/// Serve on an already-bound listener. Split out from [`serve`] so
/// integration tests can bind to `127.0.0.1:0`, read back the assigned
/// port, and hand the listener here without a bind-then-race.
pub async fn serve_on_listener(
    listener: tokio::net::TcpListener,
    state: Arc<AppState>,
) -> anyhow::Result<()> {
    serve_on_listener_until(listener, state, std::future::pending()).await.map(|_| ())
}

/// [`serve_on_listener`] with a shutdown `signal`. When it fires the drain
/// of PRD-mcphost-graceful-drain-contract runs: refuse new `/mcp` POSTs, let
/// accepted calls finish up to `MCPHOST_DRAIN_S`, interrupt the rest, write one
/// `restarts` row. `Ok(true)` means a drain ran (the caller exits 0).
pub async fn serve_on_listener_until(
    listener: tokio::net::TcpListener,
    state: Arc<AppState>,
    signal: impl std::future::Future<Output = ()> + Send + 'static,
) -> anyhow::Result<bool> {
    // PRD-mcphost-tools-list-alias-truth AC4: two tools whose flattened
    // names collide stop the server before it accepts a connection.
    crate::handler::validate_tool_registry(&state.kinds)?;
    // PRD-mcphost-oauth-metadata-lists-hosted-as R4: once per process, name
    // stored issuers the metadata documents now omit.
    if let Err(e) = crate::oauth::warn_invalid_stored_issuers(&state).await {
        tracing::warn!(error = %e, "could not scan stored OAuth issuers at startup");
    }
    let db = state.db.clone();
    let app = build_router(state);
    tracing::info!(addr = ?listener.local_addr().ok(), "mcphost listening");
    let drain = crate::drain::global();
    let bound_s = crate::drain::bound_s_from_env();
    tokio::spawn(async move {
        signal.await;
        let info = drain.begin(bound_s);
        tracing::info!(restart_id = %info.restart_id, in_flight = info.in_flight_at_signal, bound_s, "drain: signal received");
    });
    let serve = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    );
    // The listener keeps answering during the drain (the accept loop is
    // polled alongside the drain below) so `/healthz` can say `draining` and
    // a new `/mcp` POST gets an explicit 503 from `drain_gate`; only the exit
    // after the drain ends it.
    let drain_flow = async {
        drain.wait_draining().await;
        let Some(info) = drain.info() else { return Ok(true) };
        let bound = std::time::Duration::from_secs(info.bound_s);
        let poll = std::time::Duration::from_millis(10);
        // Accepted calls run on until none is left or the bound expires.
        while drain.in_flight() > 0 && info.at.elapsed() < bound {
            tokio::time::sleep(poll).await;
        }
        if drain.in_flight() > 0 {
            drain.cancel();
            // Interrupted calls unwind straight away; allow them a moment to
            // record their `calls` row and answer.
            let unwind = std::time::Duration::from_secs(2);
            let t = Instant::now();
            while drain.in_flight() > 0 && t.elapsed() < unwind {
                tokio::time::sleep(poll).await;
            }
        }
        let drain_ms = info.at.elapsed().as_millis() as i64;
        // Let the finished and interrupted responses reach their sockets
        // before the process exits.
        if info.in_flight_at_signal > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        // PRD-mcphost-graceful-drain-contract-b R3: an async run still
        // `running` now reads `interrupted` before the process exits.
        match db.interrupt_running_runs().await {
            Ok(n) if n > 0 => tracing::info!(runs = n, "drain: interrupted running async runs"),
            Ok(_) => {}
            Err(e) => tracing::error!(error = %e, "drain: could not interrupt running runs"),
        }
        let dropped = drain.interrupted() as i64;
        let in_flight_at_signal = info.in_flight_at_signal as i64;
        let row = crate::db::RestartRow {
            restart_id: info.restart_id.clone(),
            signal_at_unix: info.signal_at_unix,
            in_flight_at_signal,
            finished: (in_flight_at_signal - dropped).max(0),
            dropped,
            drain_ms,
            bound_s: info.bound_s as i64,
            version: env!("CARGO_PKG_VERSION").to_string(),
        };
        tracing::info!(
            restart_id = %row.restart_id, in_flight_at_signal = row.in_flight_at_signal,
            finished = row.finished, dropped = row.dropped, drain_ms = row.drain_ms,
            bound_s = row.bound_s, version = %row.version, "restart"
        );
        // Never block shutdown on storage: a failed write logs and we still exit.
        if let Err(e) = db.record_restart(row).await {
            tracing::error!(error = %e, "restart row write failed");
        }
        Ok::<bool, anyhow::Error>(true)
    };
    tokio::select! {
        r = serve.into_future() => {
            r?;
            Ok(false)
        }
        r = drain_flow => r,
    }
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
        return serve_until_signal(listener, state).await;
    }
    tracing::info!(%bind, "mcphost: binding by address");
    let listener = tokio::net::TcpListener::bind(bind).await?;
    serve_until_signal(listener, state).await
}

/// PRD-mcphost-graceful-drain-contract R1: serve until SIGTERM/SIGINT, drain,
/// then exit 0 (a drained process does not wait on the runtime's background
/// tasks).
async fn serve_until_signal(
    listener: tokio::net::TcpListener,
    state: Arc<AppState>,
) -> anyhow::Result<()> {
    use tokio::signal::unix::{SignalKind, signal};
    // Installed before the first accept so a signal can never fall in a gap.
    let mut term = signal(SignalKind::terminate())?;
    let mut int = signal(SignalKind::interrupt())?;
    let shutdown = async move {
        tokio::select! {
            _ = term.recv() => {}
            _ = int.recv() => {}
        }
    };
    if serve_on_listener_until(listener, state, shutdown).await? {
        std::process::exit(0);
    }
    Ok(())
}

crate::err_code! {
    pub AUTHORIZATION_REQUIRED,
    code: "authorization_required", family: Oauth, class: HostRefusal, retryable: false,
    remedy: "Authorize through the OAuth metadata named in the WWW-Authenticate header, then retry with the token.",
}


#[cfg(test)]
mod anon_healthz_tests {
    use super::*;
    use crate::build_info::BUILD;

    /// Runs in release builds too (no `debug_assert`): the body the handler
    /// serves for either liveness verdict has exactly `ANON_HEALTHZ_KEYS`.
    #[test]
    fn anon_body_keys_equal_the_const() {
        for ok in [true, false] {
            let body = anon_healthz_body(ok, &BUILD);
            assert_eq!(check_anon_healthz_keys(&body), Ok(()));
            assert_eq!(body["ok"], json!(ok));
        }
    }

    #[test]
    fn an_extra_key_is_reported_naming_the_const() {
        let mut body = anon_healthz_body(true, &BUILD);
        body.insert("extra".into(), json!(1));
        let err = check_anon_healthz_keys(&body).unwrap_err();
        assert!(err.contains("ANON_HEALTHZ_KEYS"), "{err}");
    }
}
