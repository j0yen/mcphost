//! Business logic for the `admin.*` tools, visible only to `$MCPHOST_ADMIN_KEY`.

use std::collections::HashMap;

use serde_json::{Value, json};

use crate::db::TenantDeleteCounts;
use crate::errors::AppError;
use crate::state::AppState;

/// PRD-mcphost-tenant-delete requirement 6: a batch dry run or delete
/// touches at most this many tenants per call.
const MAX_PREFIX_BATCH: i64 = 500;
/// PRD-mcphost-tenant-delete requirement 2 / AC5: a prefix under this many
/// characters (empty included) is refused, so a typo cannot empty the box.
const MIN_PREFIX_LEN: usize = 4;

/// PRD-mcphost-admin-schema-contract requirement 1: the row shape
/// `admin.tenants`/`admin.usage` carry today, pinned by
/// `schemas/admin/tenants.v1.json`/`schemas/admin/usage.v1.json`. A change
/// to either listing's row fields must bump this alongside a schema file
/// change, or `mcphost_admin_schema_contract_ac06_*` fails naming the drift.
pub const SCHEMA_VERSION: i64 = 1;

fn arg_str(args: &Value, name: &str) -> Result<String, AppError> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AppError::InvalidArgs(format!("missing required argument '{name}'")))
}

fn arg_str_opt(args: &Value, name: &str) -> Option<String> {
    args.get(name).and_then(Value::as_str).map(str::to_string)
}

/// PRD-mcphost-abuse-guard-ban-list: `admin.ban.remove`'s required `id`.
fn arg_i64(args: &Value, name: &str) -> Result<i64, AppError> {
    args.get(name)
        .and_then(Value::as_i64)
        .ok_or_else(|| AppError::InvalidArgs(format!("missing required argument '{name}'")))
}

/// PRD-mcphost-tenant-tables requirement 5 (AC7): removes a deleted
/// tenant's whole `host.table.*` store -- one file removal (plus its WAL/
/// SHM sidecars), not a set of `DELETE ... WHERE tenant_id` statements that
/// could miss a table, per the SQLite-per-tenant design `tables.rs`'s
/// module doc settles on. Best-effort: a tenant that never created a table
/// has no file to remove, and a removal failure here must never fail (or
/// roll back) the tenant delete that already committed in `Db::delete_tenant`.
fn remove_tenant_tables(state: &AppState, tenant_id: i64) {
    let base = state.db.data_dir().join("tables").join(format!("{tenant_id}.db"));
    for suffix in ["", "-wal", "-shm"] {
        let path = if suffix.is_empty() {
            base.clone()
        } else {
            let mut p = base.clone().into_os_string();
            p.push(suffix);
            p.into()
        };
        let _ = std::fs::remove_file(path);
    }
}

/// AC10 (P1 requirement 7): `admin.tenants(prefix?)` -- with no `prefix`,
/// the original unfiltered list; with one, the same `display_name`-prefix
/// filter the batch delete uses, minus its counts, so an operator can see
/// what a batch would target before running `tenant_delete_by_prefix`.
///
/// PRD-mcphost-synthetic-flag requirement 4 / AC8: every row carries
/// `synthetic` (null for unlabeled) -- this is the listing `tenants.jsonl`-style
/// exports read, so the field rides along without a second query.
/// PRD-mcphost-synthetic-flag P1 requirement 5 / AC9: `synthetic: "true"|"false"|"all"`
/// (default `"all"`) filters the same rows in-process, after the `prefix`
/// filter above, rather than a second SQL predicate -- both filters already
/// fetch full `Tenant` rows, so composing them here avoids a query variant
/// per combination.
pub async fn tenants(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let prefix = arg_str_opt(args, "prefix");
    let synthetic_filter = arg_str_opt(args, "synthetic").unwrap_or_else(|| "all".to_string());
    let rows = match prefix {
        Some(p) => state.db.list_tenants_by_prefix(p, None).await?.0,
        None => state.db.list_tenants().await?,
    };
    let tenants: Vec<Value> = rows
        .into_iter()
        .filter(|t| match synthetic_filter.as_str() {
            "true" => t.synthetic.is_some(),
            "false" => t.synthetic.is_none(),
            _ => true,
        })
        .map(|t| {
            json!({
                "tenant": t.namespace,
                "display_name": t.display_name,
                "created_at": t.created_at,
                "disabled": t.disabled,
                // PRD-mcphost-tenant-self-offboard P1 requirement 5 / AC5:
                // `"self_offboard"` vs `"admin_disable"` (or `null` for an
                // enabled tenant, or one disabled before this column
                // existed) -- an operator can tell the two apart without a
                // second query.
                "disabled_reason": t.disabled_reason,
                "synthetic": t.synthetic,
                // PRD-mcphost-tenant-attribution: rides along with the
                // export the same way `synthetic` already does -- an
                // operator auditing this listing shouldn't need a second
                // query to see how a row was classified or what client it
                // recorded.
                "source_class": t.source_class,
                "client_name": t.client_name,
                "client_version": t.client_version,
                // PRD-mcphost-human-claim-magic-link requirement 6 / AC9:
                // additive, boolean only -- never `owner_email` itself, so
                // an operator can see claim progress in this listing
                // without this row becoming a second place a human's
                // address is stored.
                "owner_verified": t.owner_verified_at.is_some(),
            })
        })
        .collect();
    Ok(json!({ "schema_version": SCHEMA_VERSION, "tenants": tenants }))
}

fn counts_json(counts: &TenantDeleteCounts) -> Value {
    json!({
        "tools_removed": counts.tools_removed,
        "secrets_removed": counts.secrets_removed,
        "calls_removed": counts.calls_removed,
        "logs_removed": counts.logs_removed,
    })
}

/// P0 requirement 1 (AC1/AC2/AC7): delete one tenant and cascade every row
/// that references it, atomically, in `Db::delete_tenant`. Structured log
/// line + `admin_events` row (requirement 4 / AC8) happen here and in
/// `Db::delete_tenant` respectively -- the log is process-observability,
/// the DB row is the durable audit trail.
pub async fn tenant_delete(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let tenant = arg_str(args, "tenant")?;
    let (deleted, counts) = state
        .db
        .delete_tenant(tenant.clone())
        .await?
        .ok_or_else(|| AppError::TenantNotFound(tenant.clone()))?;
    // PRD-mcphost-code-tools-warm-pool requirement 2 / AC3: same guarantee
    // as `tenant_disable`, for an outright delete.
    for k in state.kinds.all() {
        k.on_tenant_removed(deleted.id).await;
    }
    // PRD-mcphost-tenant-tables requirement 5 / AC7: the cascade the
    // `tenants` table's own foreign keys can't reach -- a tenant's table
    // store is a file, not a row.
    remove_tenant_tables(state, deleted.id);
    tracing::info!(
        tenant = %deleted.namespace,
        action = "tenant_delete",
        dry_run = false,
        tools_removed = counts.tools_removed,
        secrets_removed = counts.secrets_removed,
        calls_removed = counts.calls_removed,
        logs_removed = counts.logs_removed,
        "admin tenant delete"
    );
    let mut result = json!({
        "tenant": deleted.namespace,
        "namespace": deleted.namespace,
    });
    if let Some(obj) = result.as_object_mut()
        && let Some(counts_obj) = counts_json(&counts).as_object()
    {
        obj.extend(counts_obj.clone());
    }
    Ok(result)
}

/// P0 requirement 2 (AC3/AC4/AC5/AC6): batch cascade-delete every tenant
/// whose `display_name` starts with `prefix`. `dry_run` (default true)
/// only previews counts; `dry_run=false` deletes one transaction per
/// tenant via the same `Db::delete_tenant` AC1 goes through, so a batch
/// delete's per-tenant behavior (cascade, audit row, log line) is
/// identical to a single delete, just looped.
pub async fn tenant_delete_by_prefix(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let prefix = arg_str(args, "prefix")?;
    let dry_run = args.get("dry_run").and_then(Value::as_bool).unwrap_or(true);

    if prefix.chars().count() < MIN_PREFIX_LEN {
        return Err(AppError::InvalidParams(format!(
            "prefix must be at least {MIN_PREFIX_LEN} characters"
        )));
    }

    let (matches, truncated) = state
        .db
        .list_tenants_by_prefix(prefix.clone(), Some(MAX_PREFIX_BATCH))
        .await?;

    if dry_run {
        let mut items = Vec::with_capacity(matches.len());
        for t in &matches {
            let (_, counts) = state
                .db
                .tenant_delete_preview(t.namespace.clone())
                .await?
                .unwrap_or((t.clone(), TenantDeleteCounts::default()));
            let mut item = json!({
                "tenant": t.namespace,
                "display_name": t.display_name,
            });
            if let (Some(obj), Some(counts_obj)) =
                (item.as_object_mut(), counts_json(&counts).as_object())
            {
                obj.extend(counts_obj.clone());
            }
            items.push(item);
        }
        tracing::info!(
            prefix = %prefix,
            action = "tenant_delete_by_prefix",
            dry_run = true,
            matched = items.len(),
            truncated,
            "admin batch tenant delete (dry run)"
        );
        return Ok(json!({
            "dry_run": true,
            "matched": items,
            "truncated": truncated,
        }));
    }

    let mut total = TenantDeleteCounts::default();
    let mut deleted = Vec::with_capacity(matches.len());
    for t in &matches {
        if let Some((deleted_tenant, counts)) = state.db.delete_tenant(t.namespace.clone()).await? {
            for k in state.kinds.all() {
                k.on_tenant_removed(deleted_tenant.id).await;
            }
            remove_tenant_tables(state, deleted_tenant.id);
            total.accumulate(&counts);
            tracing::info!(
                tenant = %deleted_tenant.namespace,
                action = "tenant_delete_by_prefix",
                dry_run = false,
                tools_removed = counts.tools_removed,
                secrets_removed = counts.secrets_removed,
                calls_removed = counts.calls_removed,
                logs_removed = counts.logs_removed,
                "admin tenant delete (batch)"
            );
            deleted.push(deleted_tenant.namespace);
        }
    }
    let mut result = json!({
        "dry_run": false,
        "deleted": deleted,
        "truncated": truncated,
    });
    if let Some(obj) = result.as_object_mut()
        && let Some(counts_obj) = counts_json(&total).as_object()
    {
        obj.extend(counts_obj.clone());
    }
    Ok(result)
}

pub async fn tenant_disable(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let tenant = arg_str(args, "tenant")?;
    // PRD-mcphost-code-tools-warm-pool requirement 2 / AC3: a disabled
    // tenant's warm sandboxes must not outlive the disable call. Looked up
    // before the flip, not after, only so the id is available even if
    // `set_tenant_disabled` somehow raced the row away between the two
    // calls -- immaterial in practice (this handler holds no lock across
    // them), just the more defensive order.
    let tenant_id = state
        .db
        .find_tenant_by_namespace(tenant.clone())
        .await?
        .map(|t| t.id);
    let changed = state.db.set_tenant_disabled(tenant.clone(), true).await?;
    if !changed {
        return Err(AppError::ToolNotFound(format!("tenant {tenant}")));
    }
    if let Some(tenant_id) = tenant_id {
        for k in state.kinds.all() {
            k.on_tenant_removed(tenant_id).await;
        }
    }
    Ok(json!({ "tenant": tenant, "disabled": true }))
}

pub async fn tenant_enable(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let tenant = arg_str(args, "tenant")?;
    let changed = state.db.set_tenant_disabled(tenant.clone(), false).await?;
    if !changed {
        return Err(AppError::ToolNotFound(format!("tenant {tenant}")));
    }
    Ok(json!({ "tenant": tenant, "disabled": false }))
}

/// AC19: mark a tenant's domain namespace as verified so
/// `host.registry_publish` will run for it. The verification METHOD (DNS
/// or HTTP) is deliberately not this crate's concern (PRD Open Questions,
/// owned by Joe) -- this tool is the minimal admin-set boolean path the
/// PRD's registry-publish requirement can be implemented against without
/// deciding that question.
pub async fn tenant_verify_namespace(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let tenant = arg_str(args, "tenant")?;
    let domain_namespace = arg_str(args, "domain_namespace")?;
    let changed = state
        .db
        .set_tenant_namespace_verified(tenant.clone(), domain_namespace.clone())
        .await?;
    if !changed {
        return Err(AppError::ToolNotFound(format!("tenant {tenant}")));
    }
    Ok(json!({ "tenant": tenant, "domain_namespace": domain_namespace, "verified": true }))
}

pub async fn usage(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let window = arg_str_opt(args, "window").unwrap_or_else(|| "24h".to_string());
    let secs = crate::state::parse_window_secs(&window);
    let rows = state.db.usage_by_tenant_and_tool(secs).await?;
    let usage: Vec<Value> = rows
        .into_iter()
        .map(|u| {
            json!({
                "tenant": u.namespace,
                "tool": u.tool_name,
                "calls": u.stats.calls,
                "errors": u.stats.errors,
                "p50_ms": u.stats.p50_ms,
                "p95_ms": u.stats.p95_ms,
            })
        })
        .collect();
    // PRD-mcphost-data-retention P0 requirement 3 (AC5): `db_bytes`,
    // `db_page_free_bytes`, `rows_by_table`, and `last_prune` --
    // observability for a database that no longer grows unbounded.
    let size = state.db.usage_size_stats().await?;
    let rows_by_table: serde_json::Map<String, Value> = size
        .rows_by_table
        .into_iter()
        .map(|(table, count)| (table, json!(count)))
        .collect();
    let last_prune = size.last_prune.map(|p| {
        json!({
            "at": crate::state::rfc3339_from_unix(p.at_unix),
            "ok": p.ok,
            "deleted": p.deleted,
        })
    });
    // PRD-mcphost-tenant-data-export P2 requirement 6 / AC7: exports run
    // today (UTC calendar day, host-wide) -- `now - now % 86_400` is
    // today's UTC midnight since the Unix epoch itself starts at one.
    let now = crate::state::now_unix();
    let today_start = now - now.rem_euclid(86_400);
    let exports_today = state
        .db
        .count_runs_by_tool_since(crate::export::EXPORT_TOOL_NAME.to_string(), today_start)
        .await?;
    // PRD-mcphost-python-dependency-policy requirement 7 (AC9): a
    // host-wide, unwindowed snapshot -- every currently-published python
    // tool counted by its own network mode, and separately by whether its
    // current lock carries any live advisory (from `tool_lock`, so a
    // re-audit's update is reflected with no republish -- same reasoning
    // as `control::tool_list`'s own `advisories` field).
    let all_tools = state.db.list_all_tool_specs().await?;
    let advisory_counts = state.db.current_tool_lock_advisory_counts().await?;
    let mut tools_by_network: HashMap<String, i64> = HashMap::new();
    let mut advisory_clean = 0i64;
    let mut advisory_flagged = 0i64;
    for (tenant_id, name, kind, spec) in &all_tools {
        if kind != "python" {
            continue;
        }
        let network = crate::kinds::python::network_mode_label(spec).unwrap_or_else(|| "none".to_string());
        *tools_by_network.entry(network).or_insert(0) += 1;
        let advisories = advisory_counts.get(&(*tenant_id, name.clone())).copied().unwrap_or(0);
        if advisories > 0 {
            advisory_flagged += 1;
        } else {
            advisory_clean += 1;
        }
    }
    let tools_by_network: serde_json::Map<String, Value> =
        tools_by_network.into_iter().map(|(k, v)| (k, json!(v))).collect();

    Ok(json!({
        "schema_version": SCHEMA_VERSION,
        "window": window,
        "usage": usage,
        "db_bytes": size.db_bytes,
        "db_page_free_bytes": size.db_page_free_bytes,
        "rows_by_table": rows_by_table,
        "last_prune": last_prune,
        "exports_today": exports_today,
        "tools_by_network": tools_by_network,
        "tools_by_advisory_state": {"clean": advisory_clean, "advisory": advisory_flagged},
    }))
}

/// PRD-mcphost-python-dependency-policy requirement 6 (AC8): run one
/// dependency re-audit cycle immediately (the same cycle the daily
/// scheduler runs) and return its summary -- same on-demand-trigger shape
/// as [`prune_now`] below.
pub async fn dependency_reaudit(state: &AppState) -> Result<Value, AppError> {
    crate::deps::reaudit_once(state).await
}

/// P2 requirement 7 (AC10): run one retention-prune cycle on demand and
/// return its per-table deleted counts -- the same [`crate::db::Db::prune_once`]
/// the nightly scheduler calls.
pub async fn prune_now(state: &AppState) -> Result<Value, AppError> {
    let report = state.db.prune_once().await?;
    Ok(json!({
        "started_unix": report.started_unix,
        "finished_unix": report.finished_unix,
        "deleted": report.deleted,
    }))
}

/// PRD-mcphost-sandbox-ready requirement 4 (AC5): re-runs the sandbox
/// self-test on demand, on every registered kind that has one (only
/// `python` does today -- see `Kind::sandbox_recheck`'s doc comment), and
/// returns the first fresh status found. Admin-key gated like every other
/// tool in this file. `sandbox_unsupported` when no registered kind has a
/// self-test at all (a deployment with no sandboxed kind registered).
pub async fn sandbox_recheck(state: &AppState) -> Result<Value, AppError> {
    for kind in state.kinds.all() {
        if let Some(status) = kind.sandbox_recheck().await {
            return Ok(json!({
                "ready": status.ready,
                "mechanism": status.mechanism.as_str(),
                "detail": status.detail,
                "checked_at": status.checked_at,
            }));
        }
    }
    Err(AppError::Structured {
        code: "sandbox_unsupported",
        message: "no registered kind on this host has a sandbox self-test".to_string(),
        data: Value::Null,
    })
}

/// `admin.billing_ledger(since?, until?, tenant?)` (AC9): every ledgered
/// billing event, newest first, capped at 1000 by `Db::list_billing_events`
/// -- read-only, and (technical considerations) the same rows the
/// `measure` command reads without ever needing Stripe credentials.
pub async fn billing_ledger(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let since = args.get("since").and_then(Value::as_i64);
    let until = args.get("until").and_then(Value::as_i64);
    let tenant = arg_str_opt(args, "tenant");
    let rows = state.db.list_billing_events(since, until, tenant).await?;
    let events: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            json!({
                "event_id": r.event_id,
                "event_type": r.event_type,
                "tenant": r.tenant,
                "plan": r.plan,
                "amount_cents": r.amount_cents,
                "currency": r.currency,
                "mode": r.mode,
                "received_at": r.received_at,
            })
        })
        .collect();
    Ok(json!({ "events": events }))
}

/// `admin.plan_set(tenant, plan, reason)`: a support override, ledgered as
/// `event_type: admin.plan_set` with `mode` = the host's current billing
/// mode (requirement: "the billing tools" / P0 admin surface) -- so a
/// support-granted upgrade shows up in the same ledger a real payment
/// would, distinguishable only by its `event_type`.
pub async fn plan_set(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let tenant_ns = arg_str(args, "tenant")?;
    let plan_name = arg_str(args, "plan")?;
    let reason = arg_str_opt(args, "reason").unwrap_or_default();

    if state.plans.get(&plan_name).is_none() {
        return Err(AppError::InvalidParams(format!(
            "unknown plan '{plan_name}'"
        )));
    }
    let tenant = state
        .db
        .find_tenant_by_namespace(tenant_ns.clone())
        .await?
        .ok_or_else(|| AppError::TenantNotFound(tenant_ns.clone()))?;

    let plan_since = crate::state::rfc3339_now();
    state
        .db
        .upgrade_tenant_plan(tenant.id, plan_name.clone(), plan_since, None)
        .await?;

    let mode = state.billing_config.billing_mode();
    // A support override is ledgered under a synthetic, always-unique
    // event id (there is no processor event backing it) so it lands in
    // the same `billing_events` table `admin.billing_ledger` reads.
    let event_id = format!(
        "admin.plan_set:{}:{}",
        tenant.namespace,
        crate::state::now_unix()
    );
    let payload = json!({"tenant": tenant.namespace, "plan": plan_name, "reason": reason});
    state
        .db
        .insert_billing_event(crate::db::BillingEventInsert {
            event_id,
            event_type: "admin.plan_set".to_string(),
            tenant_id: Some(tenant.id),
            plan: Some(plan_name.clone()),
            amount_cents: None,
            currency: None,
            mode: if mode == "off" { "test".to_string() } else { mode.to_string() },
            payload_sha256: crate::billing::sha256_hex(payload.to_string().as_bytes()),
        })
        .await?;

    Ok(json!({ "tenant": tenant.namespace, "plan": plan_name }))
}

/// `admin.meter_status` (P1 AC11): last ledgered batch span, current
/// `meter_lag`, and per-tenant emitted counts for the current UTC month --
/// the operator-facing view of `mcphost billing emit-meter`'s health that
/// AC9's timer runs unattended.
pub async fn meter_status(state: &AppState) -> Result<Value, AppError> {
    let (last_call_id, _updated_at) = state.db.get_meter_state().await?;
    let lag = state.db.meter_lag(last_call_id).await?;
    let last_batch = state.db.last_meter_batch_span().await?;
    let month_start = crate::state::utc_month_start_unix(crate::state::now_unix());
    let by_tenant = state.db.monthly_emitted_counts_by_tenant(month_start).await?;
    Ok(json!({
        "last_batch": last_batch.map(|b| json!({
            "batch_id": b.batch_id,
            "first_call_id": b.first_call_id,
            "last_call_id": b.last_call_id,
            "count": b.count,
            "created_at": b.created_at,
        })),
        "meter_lag": lag,
        "emitted_this_month": by_tenant
            .into_iter()
            .map(|(namespace, count)| json!({"tenant": namespace, "count": count}))
            .collect::<Vec<_>>(),
    }))
}

pub async fn tool_list(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let tenant_ns = arg_str(args, "tenant")?;
    let tenant = state
        .db
        .find_tenant_by_namespace(tenant_ns.clone())
        .await?
        .ok_or_else(|| AppError::ToolNotFound(format!("tenant {tenant_ns}")))?;
    let rows = state.db.list_tools(tenant.id).await?;
    let tools: Vec<Value> = rows
        .into_iter()
        .map(|row| {
            // PRD-mcphost-python-kind-plain-env requirement 6 (AC10, P2,
            // best effort): env *names* and their total byte size, never
            // values -- an operator reviewing a tenant's tools this way
            // must be able to see how much plain configuration a tool
            // carries without ever seeing what it says.
            let env_map = state
                .kinds
                .get(&row.kind)
                .map(|kind| kind.env_map(&row.spec))
                .unwrap_or_default();
            let env_total_bytes: usize =
                env_map.iter().map(|(k, v)| k.len() + v.len()).sum();
            json!({
                "name": format!("{}.{}", tenant.namespace, row.name),
                "kind": row.kind,
                "created_at": row.created_at,
                "env_names": env_map.into_keys().collect::<Vec<_>>(),
                "env_total_bytes": env_total_bytes,
            })
        })
        .collect();
    Ok(json!({ "tenant": tenant_ns, "tools": tools }))
}

/// PRD-mcphost-synthetic-flag requirement 3 / AC5: `label` is required in
/// the schema but its *value* carries the set/clear distinction -- JSON
/// `null` clears, a valid string sets, anything else (missing key,
/// non-string/non-null value, or a string failing
/// [`crate::state::is_valid_synthetic_label`]) is rejected outright. Unlike
/// the signup header (requirement 2: invalid degrades to null, signup still
/// succeeds), an admin explicitly calling this tool with a bad label gets
/// an error, not a silent no-op.
fn arg_synthetic_label(args: &Value) -> Result<Option<String>, AppError> {
    match args.get("label") {
        None => Err(AppError::InvalidArgs(
            "missing required argument 'label'".to_string(),
        )),
        Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => {
            if crate::state::is_valid_synthetic_label(s) {
                Ok(Some(s.clone()))
            } else {
                Err(AppError::InvalidParams(format!(
                    "invalid synthetic label '{s}'"
                )))
            }
        }
        Some(_) => Err(AppError::InvalidArgs(
            "'label' must be a string or null".to_string(),
        )),
    }
}

/// `admin.tenant_set_synthetic(tenant, label)` (AC5): set or clear one
/// tenant's `synthetic` label by namespace -- same identifier every other
/// single-tenant admin tool uses.
pub async fn tenant_set_synthetic(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let tenant = arg_str(args, "tenant")?;
    let label = arg_synthetic_label(args)?;
    let changed = state
        .db
        .set_tenant_synthetic(tenant.clone(), label.clone())
        .await?;
    if !changed {
        return Err(AppError::ToolNotFound(format!("tenant {tenant}")));
    }
    tracing::info!(
        tenant = %tenant,
        action = "tenant_set_synthetic",
        label = label.as_deref().unwrap_or(""),
        "admin set tenant synthetic label"
    );
    Ok(json!({ "tenant": tenant, "synthetic": label }))
}

/// `admin.tenants_set_synthetic(name_like, label, dry_run)` (AC6): the
/// current-census backfill recipe (README operator section) runs this
/// twice -- `name_like: 'joe-%'` / `label: 'operator'`, then
/// `name_like: '%'` (or a narrower panel-matching pattern) /
/// `label: 'synthorg:backfill-20260906'` -- each call previewed with
/// `dry_run: true` before the `dry_run: false` that applies it.
pub async fn tenants_set_synthetic(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let name_like = arg_str(args, "name_like")?;
    let label = arg_str(args, "label")?;
    if !crate::state::is_valid_synthetic_label(&label) {
        return Err(AppError::InvalidParams(format!(
            "invalid synthetic label '{label}'"
        )));
    }
    let dry_run = args.get("dry_run").and_then(Value::as_bool).unwrap_or(true);

    let matches = state
        .db
        .set_tenants_synthetic_by_name_like(name_like.clone(), label.clone(), dry_run)
        .await?;
    let matched: Vec<Value> = matches
        .iter()
        .map(|t| {
            json!({
                "tenant": t.namespace,
                "display_name": t.display_name,
                "synthetic": t.synthetic,
            })
        })
        .collect();
    tracing::info!(
        name_like = %name_like,
        action = "tenants_set_synthetic",
        dry_run,
        matched = matched.len(),
        label = %label,
        "admin bulk synthetic tag"
    );
    Ok(json!({
        "dry_run": dry_run,
        "label": label,
        "count": matched.len(),
        "matched": matched,
    }))
}

/// PRD-mcphost-provenance-audit requirement 4: which `admin.*` tools are
/// mutations worth an audit row, and how to name their target/detail.
/// Read-only tools are deliberately absent -- auditing a read doesn't serve
/// the "who changed what" trail this log exists for.
pub fn admin_audit_entry(
    name: &str,
    args: &Value,
    _result: &Value,
) -> Option<(String, Option<String>, Option<String>)> {
    let dry_run = args.get("dry_run").and_then(Value::as_bool).unwrap_or(false);
    match name {
        "admin.tenant_delete" => Some((
            "tenant_delete".into(),
            args.get("tenant").and_then(Value::as_str).map(String::from),
            None,
        )),
        "admin.tenant_delete_by_prefix" if !dry_run => Some((
            "tenant_delete_by_prefix".into(),
            args.get("prefix").and_then(Value::as_str).map(String::from),
            None,
        )),
        "admin.tenant_disable" => Some((
            "tenant_disable".into(),
            args.get("tenant").and_then(Value::as_str).map(String::from),
            None,
        )),
        "admin.tenant_enable" => Some((
            "tenant_enable".into(),
            args.get("tenant").and_then(Value::as_str).map(String::from),
            None,
        )),
        "admin.tenant_verify_namespace" => Some((
            "tenant_verify_namespace".into(),
            args.get("tenant").and_then(Value::as_str).map(String::from),
            args.get("domain_namespace")
                .and_then(Value::as_str)
                .map(String::from),
        )),
        "admin.plan_set" => Some((
            "plan_set".into(),
            args.get("tenant").and_then(Value::as_str).map(String::from),
            args.get("plan").and_then(Value::as_str).map(String::from),
        )),
        "admin.tenant_set_synthetic" => Some((
            "tenant_set_synthetic".into(),
            args.get("tenant").and_then(Value::as_str).map(String::from),
            None,
        )),
        "admin.tenants_set_synthetic" if !dry_run => Some((
            "tenants_set_synthetic".into(),
            args.get("name_like").and_then(Value::as_str).map(String::from),
            args.get("label").and_then(Value::as_str).map(String::from),
        )),
        // PRD-mcphost-sharing AC9: an admin-forced unshare is a mutation
        // like any other admin.* write above.
        "admin.tool_unshare" => Some((
            "tool_unshare".into(),
            args.get("tenant")
                .and_then(Value::as_str)
                .zip(args.get("name").and_then(Value::as_str))
                .map(|(t, n)| format!("{t}.{n}")),
            None,
        )),
        // PRD-mcphost-agent-mesh-ops: freeze/unfreeze/purge are mutations
        // like every other admin.* write above, alongside their own
        // dedicated `admin_events` rows (`Db::mesh_freeze`/`mesh_unfreeze`/
        // `mesh_purge`) -- this is the separate, actor-tracked `admin_audit`
        // log requirement 4 already appends every admin.* mutation to.
        "admin.mesh.freeze" => Some((
            "mesh_freeze".into(),
            args.get("tenant").and_then(Value::as_str).map(String::from),
            args.get("reason").and_then(Value::as_str).map(String::from),
        )),
        "admin.mesh.unfreeze" => Some((
            "mesh_unfreeze".into(),
            args.get("tenant").and_then(Value::as_str).map(String::from),
            None,
        )),
        "admin.mesh.purge" if !dry_run => Some((
            "mesh_purge".into(),
            None,
            args.get("older_than_days")
                .and_then(Value::as_i64)
                .map(|n| n.to_string()),
        )),
        // PRD-mcphost-abuse-guard-ban-list requirement 3 / AC7: both
        // `admin.ban.add` and `admin.ban.remove` are mutations like every
        // other admin.* write above -- `admin.ban.list` (read-only) is
        // absent, same convention `admin.tenants`/`admin.audit_log` follow.
        "admin.ban.add" => Some((
            "ban_add".into(),
            args.get("subject").and_then(Value::as_str).map(String::from),
            args.get("reason").and_then(Value::as_str).map(String::from),
        )),
        "admin.ban.remove" => Some((
            "ban_remove".into(),
            args.get("id").and_then(Value::as_i64).map(|n| n.to_string()),
            None,
        )),
        "admin.oauth.jwks_refresh" => Some((
            "oauth_jwks_refresh".into(),
            args.get("issuer").and_then(Value::as_str).map(String::from),
            None,
        )),
        // PRD-mcphost-oauth-resource-server AC9: unlike every other
        // read-only admin.* listing (admin.tenants, admin.ban.list), this
        // one is required to record admin_audit on every call, mutation or
        // not.
        "admin.oauth.issuers" => Some(("oauth_issuers_list".into(), None, None)),
        _ => None,
    }
}

/// `admin.audit_log(limit?, before_id?)` (requirement 4): paged, newest
/// first, read-only view of `admin_audit`.
pub async fn audit_log(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let limit = args
        .get("limit")
        .and_then(Value::as_i64)
        .unwrap_or(100)
        .clamp(1, 500);
    let before_id = args.get("before_id").and_then(Value::as_i64);
    let rows = state.db.list_admin_audit(limit, before_id).await?;
    let entries: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            json!({
                "id": r.id,
                "actor_key_id": r.actor_key_id,
                "action": r.action,
                "target": r.target,
                "detail": r.detail,
                "created_unix": r.created_unix,
            })
        })
        .collect();
    Ok(json!({ "entries": entries }))
}

/// `admin.shared_tools()` (PRD-mcphost-sharing user story "Operator (Joe)":
/// every currently-shared tool across every tenant, with per-day caller
/// counts).
pub async fn shared_tools(state: &AppState) -> Result<Value, AppError> {
    let tools = state.db.admin_shared_tools().await?;
    Ok(json!({ "tools": tools }))
}

/// `admin.tool_unshare(tenant, name)` (AC9): forces a shared tool back to
/// private, stamping `unshared_by: admin` so the owner's own
/// `host.tool_list` shows why.
pub async fn tool_unshare(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let tenant_ns = arg_str(args, "tenant")?;
    let name = arg_str(args, "name")?;
    let ok = state
        .db
        .admin_unshare_tool(tenant_ns.clone(), name.clone())
        .await?;
    if !ok {
        return Err(AppError::ToolNotFound(format!("{tenant_ns}.{name}")));
    }
    Ok(json!({ "tenant": tenant_ns, "name": name, "visibility": "private", "unshared_by": "admin" }))
}

// ---- mesh ops (PRD-mcphost-agent-mesh-ops) --------------------------------

fn mesh_counts_json(c: &crate::db::MeshCounts) -> Value {
    json!({"real": c.real, "synthetic": c.synthetic})
}

fn mesh_tenant_stats_json(t: &crate::db::MeshTenantStats) -> Value {
    json!({
        "tenant": t.tenant,
        "synthetic": t.synthetic,
        "messages": t.messages,
        "channel_posts": t.channel_posts,
        "contact_requests": t.contact_requests,
        "urgent": t.urgent,
        "refusals_by_code": t.refusals_by_code,
    })
}

/// `admin.mesh.stats(window, tenant?)` (P0 requirement 1 / AC1; P1
/// requirement 8 / AC9).
pub async fn mesh_stats(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let window = arg_str_opt(args, "window").unwrap_or_else(|| "1h".to_string());
    let since_ms = crate::state::now_unix_ms() - crate::state::parse_window_secs(&window) * 1000;
    let tenant = arg_str_opt(args, "tenant");
    let stats = state.db.mesh_stats(since_ms, tenant).await?;
    Ok(json!({
        "window": window,
        "messages": mesh_counts_json(&stats.messages),
        "channel_posts": mesh_counts_json(&stats.channel_posts),
        "contact_requests": mesh_counts_json(&stats.contact_requests),
        "urgent": mesh_counts_json(&stats.urgent),
        "refusals_by_code": stats.refusals_by_code,
        "wake_runs": stats.wake_runs,
        "active_pairs": stats.active_pairs,
        "active_channels": stats.active_channels,
        "tenants": stats.tenants.iter().map(mesh_tenant_stats_json).collect::<Vec<_>>(),
    }))
}

/// `admin.mesh.threads(tenant?, channel?, limit≤100)` (P0 requirement 2 /
/// AC2): no `body` field anywhere in this response.
pub async fn mesh_threads(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(100).clamp(1, 100);
    let tenant_ns = arg_str_opt(args, "tenant");
    let channel = arg_str_opt(args, "channel");
    let tenant_id = match &tenant_ns {
        Some(ns) => Some(
            state
                .db
                .find_tenant_by_namespace(ns.clone())
                .await?
                .ok_or_else(|| AppError::TenantNotFound(ns.clone()))?
                .id,
        ),
        None => None,
    };
    let (threads, channels) = state.db.mesh_threads(tenant_id, channel, limit).await?;
    Ok(json!({
        "threads": threads.iter().map(|t| json!({
            "thread_id": t.thread_id,
            "participants": t.participants,
            "message_count": t.message_count,
            "last_activity": t.last_activity,
        })).collect::<Vec<_>>(),
        "channels": channels.iter().map(|c| json!({
            "channel_id": c.channel_id,
            "name": c.name,
            "post_count": c.post_count,
            "last_activity": c.last_activity,
        })).collect::<Vec<_>>(),
    }))
}

/// `admin.mesh.thread(thread_or_channel_id, limit≤100, cursor?, reason?)`
/// (P0 requirement 3 / AC3): every call writes one `admin_events` row
/// `{action: "mesh.thread_read", target, reason?}`, whether the id names a
/// thread or a channel, and whether or not it resolves to anything at all
/// (an operator's failed lookup is itself worth auditing).
pub async fn mesh_thread(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let target = arg_str(args, "thread_or_channel_id")?;
    let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(50).clamp(1, 100);
    let reason = arg_str_opt(args, "reason");
    let cursor = args
        .get("cursor")
        .and_then(Value::as_str)
        .map(|c| c.parse::<i64>())
        .transpose()
        .map_err(|_| AppError::InvalidArgs("cursor: malformed".to_string()))?;

    state
        .db
        .record_mesh_thread_read(target.clone(), reason)
        .await?;

    if let Some(rows) = state.db.admin_thread_messages(target.clone(), cursor, limit).await? {
        let next_cursor = if rows.len() as i64 == limit {
            rows.last().map(|r| r.seq.to_string())
        } else {
            None
        };
        return Ok(json!({
            "kind": "thread",
            "thread_id": target,
            "messages": rows.iter().map(|r| json!({
                "message_id": r.id,
                "thread_id": r.thread_id,
                "seq": r.seq,
                "from_address": r.from_address,
                "body": r.body,
                "data": r.data,
                "in_reply_to": r.in_reply_to,
                "synthetic": r.synthetic,
                "source_class": r.source_class,
                "created_at": r.created_at,
                "urgent": r.urgent,
            })).collect::<Vec<_>>(),
            "next_cursor": next_cursor,
        }));
    }

    let Some((channel_id, rows)) = state.db.admin_channel_posts(target.clone(), cursor, limit).await? else {
        return Err(AppError::thread_not_found());
    };
    let next_cursor = if rows.len() as i64 == limit {
        rows.last().map(|r| r.seq.to_string())
    } else {
        None
    };
    Ok(json!({
        "kind": "channel",
        "channel_id": channel_id,
        "posts": rows.iter().map(|r| json!({
            "post_id": r.id,
            "channel_id": r.channel_id,
            "seq": r.seq,
            "from_address": r.from_address,
            "body": r.body,
            "data": r.data,
            "synthetic": r.synthetic,
            "source_class": r.source_class,
            "created_at": r.created_at,
        })).collect::<Vec<_>>(),
        "next_cursor": next_cursor,
    }))
}

/// `admin.mesh.freeze(tenant, reason)` (P0 requirement 4 / AC4, AC5).
pub async fn mesh_freeze(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let tenant = arg_str(args, "tenant")?;
    let reason = arg_str_opt(args, "reason");
    let frozen = state
        .db
        .mesh_freeze(tenant.clone(), reason)
        .await?
        .ok_or_else(|| AppError::TenantNotFound(tenant.clone()))?;
    Ok(json!({ "tenant": frozen.namespace, "mesh_frozen": true }))
}

/// `admin.mesh.unfreeze(tenant)` (P0 requirement 4 / AC5).
pub async fn mesh_unfreeze(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let tenant = arg_str(args, "tenant")?;
    let unfrozen = state
        .db
        .mesh_unfreeze(tenant.clone())
        .await?
        .ok_or_else(|| AppError::TenantNotFound(tenant.clone()))?;
    Ok(json!({ "tenant": unfrozen.namespace, "mesh_frozen": false }))
}

/// `admin.mesh.purge(older_than_days≥1, dry_run=true|false)` (P0
/// requirement 5 / AC6).
pub async fn mesh_purge(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let older_than_days = args
        .get("older_than_days")
        .and_then(Value::as_i64)
        .ok_or_else(|| AppError::InvalidArgs("missing required argument 'older_than_days'".to_string()))?;
    if older_than_days < 1 {
        return Err(AppError::InvalidParams(
            "older_than_days must be at least 1".to_string(),
        ));
    }
    let dry_run = args.get("dry_run").and_then(Value::as_bool).unwrap_or(true);
    let cutoff_unix_ms = crate::state::now_unix_ms() - older_than_days * 86_400_000;
    let counts = state.db.mesh_purge(cutoff_unix_ms, dry_run).await?;
    Ok(json!({
        "dry_run": dry_run,
        "older_than_days": older_than_days,
        "messages_removed": counts.messages_removed,
        "channel_posts_removed": counts.channel_posts_removed,
    }))
}

// ---- bans (PRD-mcphost-abuse-guard-ban-list) ------------------------------

fn ban_json(b: &crate::db::Ban) -> Value {
    json!({
        "id": b.id,
        "subject_kind": b.subject_kind,
        "subject": b.subject,
        "reason": b.reason,
        "public": b.public,
        "created_at": b.created_at,
        "created_by": b.created_by,
        "expires_at": b.expires_at,
        "auto": b.auto,
        "hits": b.hits,
        // AC12: a removed ban stays in the listing's history, stamped with
        // when it was lifted; `active` is the same "still enforcing?"
        // decision `active_only: true` filters on, spelled out per row so
        // the operator reading the history does not have to recompute it.
        "removed_at": b.removed_at,
        "active": b.removed_at.is_none()
            && b.expires_at.is_none_or(|e| e > crate::state::now_unix()),
    })
}

/// The operator identity every `admin.*` mutation is recorded under --
/// there is no per-operator auth on top of the single shared
/// `$MCPHOST_ADMIN_KEY` (same identity `handler::dispatch_admin_tool`
/// computes for `admin_audit.actor_key_id`), so a ban's own `created_by`
/// uses the identical hash rather than inventing a second notion of "who".
fn operator_identity(state: &AppState) -> String {
    state.admin_key.as_deref().map(crate::auth::hash_key).unwrap_or_default()
}

/// `admin.ban.add {subject_kind, subject, ttl|permanent, reason, public}`
/// (requirement 3 / AC1-3, AC9): `subject_kind` must be one of `key`,
/// `addr`, `email_domain`; exactly one of `ttl` (`"30m"`/`"24h"`/`"7d"`) or
/// the literal `permanent: true` is required (AC9 -- neither is a
/// validation error, and so is both at once, since they disagree about
/// whether this ban ever expires). `subject` is normalized to its at-rest
/// form (`bans::normalize_subject` -- the sha256 hex of the raw key for
/// `subject_kind: "key"`, unchanged otherwise) before it's ever stored or
/// cached, same "never a credential in the clear" rule `tenants.key_hash`
/// already follows.
pub async fn ban_add(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let subject_kind = arg_str(args, "subject_kind")?;
    if !matches!(subject_kind.as_str(), "key" | "addr" | "email_domain") {
        return Err(AppError::InvalidParams(format!(
            "subject_kind must be one of \"key\", \"addr\", \"email_domain\"; got '{subject_kind}'"
        )));
    }
    let raw_subject = arg_str(args, "subject")?;
    let reason = arg_str(args, "reason")?;
    let public = args.get("public").and_then(Value::as_bool).unwrap_or(false);
    let permanent = args.get("permanent").and_then(Value::as_bool) == Some(true);
    let ttl = arg_str_opt(args, "ttl");

    let expires_at = match (&ttl, permanent) {
        (Some(_), true) => {
            return Err(AppError::InvalidParams(
                "ttl and permanent: true are mutually exclusive".to_string(),
            ));
        }
        (None, false) => {
            return Err(AppError::InvalidParams(
                "either ttl (\"30m\", \"24h\", \"7d\") or the literal permanent: true is required"
                    .to_string(),
            ));
        }
        (None, true) => None,
        (Some(ttl), false) => {
            let secs = crate::bans::parse_ban_ttl_secs(ttl).ok_or_else(|| {
                AppError::InvalidParams(format!(
                    "ttl must look like \"30m\", \"24h\", or \"7d\"; got '{ttl}'"
                ))
            })?;
            Some(crate::state::now_unix() + secs)
        }
    };

    let subject = crate::bans::normalize_subject(&subject_kind, &raw_subject);
    let id = state
        .db
        .insert_ban(
            subject_kind.clone(),
            subject,
            reason.clone(),
            public,
            operator_identity(state),
            expires_at,
            false,
        )
        .await?;
    state.bans.refresh(&state.db).await?;
    Ok(json!({
        "id": id,
        "subject_kind": subject_kind,
        "subject": raw_subject,
        "reason": reason,
        "public": public,
        "expires_at": expires_at,
    }))
}

/// `admin.ban.remove {id}` (AC7, AC12): stops the ban enforcing at once
/// (the row is stamped `removed_at` and the cache reloaded without it) and
/// keeps it in `admin.ban.list`'s history, so a listing shows both the ban
/// and its removal -- AC12's operator check reads the outcome of both
/// actions off the same listing.
pub async fn ban_remove(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let id = arg_i64(args, "id")?;
    let removed = state.db.remove_ban(id).await?;
    if !removed {
        return Err(AppError::ToolNotFound(format!("ban {id}")));
    }
    state.bans.refresh(&state.db).await?;
    Ok(json!({ "id": id, "removed": true }))
}

/// `admin.ban.list {active_only?, subject_kind?}` (AC8, AC12): the default
/// (`active_only` absent or false) is the history -- expired and removed
/// rows included, each carrying `expires_at`/`removed_at`/`active`.
pub async fn ban_list(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let active_only = args.get("active_only").and_then(Value::as_bool).unwrap_or(false);
    let subject_kind = arg_str_opt(args, "subject_kind");
    let rows = state.db.list_bans(active_only, subject_kind).await?;
    Ok(json!({ "bans": rows.iter().map(ban_json).collect::<Vec<_>>() }))
}
