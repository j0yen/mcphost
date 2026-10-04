//! PRD-mcphost-row-policy: one policy compiles to a SQL row filter and a
//! docs filter under one hash, with a hash-chained audit. Grounding names
//! ai-stack's `policy.rs`/`audit.rs`/`context.rs`/`gateway.rs` (commit
//! 4ef6c22, aistack-security) as the port source; that tree was not
//! present on this build host, so `policy.rs`/`audit.rs`/`rewrite.rs` here
//! reimplement the same shapes and contracts described in the PRD rather
//! than copying source lines.
//!
//! This module owns the policy model and pure compile/audit/rewrite logic
//! (`policy.rs`, `audit.rs`, `rewrite.rs`) plus the `host.policy.*`/
//! `host.audit.*` tool entrypoints below -- the same "business logic lives
//! with its tools" convention `vault.rs`/`enduser.rs` already use.

pub mod audit;
pub mod policy;
pub mod rewrite;

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::db::Tenant;
use crate::enduser::EndUser;
use crate::errors::AppError;
use crate::state::{AppState, now_unix};

pub use policy::{CompiledPolicy, PolicyRule, PolicyTarget, RowPolicy, RuleOp, RuleValue, SecurityContext};

/// requirement 3: `Unrestricted` (a tenant-key call, or any call with no
/// end user) applies no filter at all; `Restricted` carries the resolved
/// [`SecurityContext`] `compile()` needs.
pub enum Access {
    Unrestricted,
    Restricted(SecurityContext),
}

/// requirement 3: builds this call's [`Access`] from its `end_user` (if
/// any) plus that subject's stored attributes. Takes a bare `tenant_id`
/// (not `&Tenant`) so a background caller with no full `Tenant` in hand
/// (e.g. `docs::rerun_search`'s own re-run path, always `end_user: None`)
/// can still call this -- the `Some(eu)` branch below is the only one that
/// needs tenant scoping at all.
pub async fn resolve_access(
    state: &AppState,
    tenant_id: i64,
    end_user: Option<&EndUser>,
) -> Result<Access, AppError> {
    let Some(eu) = end_user else {
        return Ok(Access::Unrestricted);
    };
    let attrs = state.db.end_user_attrs_get(tenant_id, eu.subject.clone()).await?;
    Ok(Access::Restricted(SecurityContext {
        subject: eu.subject.clone(),
        issuer: eu.issuer.clone(),
        method: eu.method.as_str().to_string(),
        attrs,
    }))
}

fn row_policy_from_record(record: crate::db::RowPolicyRecord, target: PolicyTarget) -> Result<RowPolicy, AppError> {
    let rule: Vec<PolicyRule> = serde_json::from_str(&record.rule_json)
        .map_err(|e| AppError::Internal(format!("stored row policy rule_json is corrupt: {e}")))?;
    Ok(RowPolicy { id: record.id, tenant_id: record.tenant_id, version: record.version, target, rule, created_unix: record.created_unix })
}

/// requirement 3 (goal 3): the table's policy, or the synthesized empty
/// policy (`version: 0`, no rule) that `compile()` always resolves to a
/// fail-closed `1 = 0` -- the "no policy configured" case.
pub async fn load_table_policy(state: &AppState, tenant_id: i64, table_name: &str) -> Result<RowPolicy, AppError> {
    match state.db.get_row_policy(tenant_id, "table".to_string(), table_name.to_string()).await? {
        Some(record) => row_policy_from_record(record, PolicyTarget::Table(table_name.to_string())),
        None => Ok(RowPolicy {
            id: 0,
            tenant_id,
            version: 0,
            target: PolicyTarget::Table(table_name.to_string()),
            rule: Vec::new(),
            created_unix: 0,
        }),
    }
}

/// requirement 5: every `doc_prefix` policy this tenant has, fetched once
/// per `host.docs.search` call so [`doc_allowed`] below can check each
/// candidate chunk against it purely in-memory rather than one DB round
/// trip per chunk.
pub async fn load_doc_prefix_policies(state: &AppState, tenant_id: i64) -> Result<Vec<RowPolicy>, AppError> {
    let records = state.db.list_row_policies(tenant_id).await?;
    records
        .into_iter()
        .filter(|r| r.target_kind == "doc_prefix")
        .map(|r| {
            let target = PolicyTarget::DocPrefix(r.target_value.clone());
            row_policy_from_record(r, target)
        })
        .collect()
}

/// requirement 5 (AC5, goal 3): is `ctx`'s subject admitted to `doc_name`
/// under the longest matching prefix policy in `policies`? A prefix with
/// no configured policy admits nothing -- "a prefix with no policy is
/// invisible to end users" -- rather than falling back to some broader
/// policy or to unrestricted.
pub fn doc_allowed(policies: &[RowPolicy], doc_name: &str, ctx: &SecurityContext) -> bool {
    let best = policies
        .iter()
        .filter(|p| doc_name.starts_with(p.target.value()))
        .max_by_key(|p| p.target.value().len());
    match best {
        Some(policy) => policy::compile(policy, ctx).docs_subject_allowed,
        None => false,
    }
}

fn arg_str(args: &Value, name: &str) -> Result<String, AppError> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AppError::InvalidArgs(format!("missing required argument '{name}'")))
}

fn arg_bool(args: &Value, name: &str) -> bool {
    args.get(name).and_then(Value::as_bool).unwrap_or(false)
}

fn describe_rule_value(value: &RuleValue) -> String {
    match value {
        RuleValue::Literal(v) => v.to_string(),
        RuleValue::Attr(name) => format!("attr({name})"),
    }
}

fn parse_target(args: &Value) -> Result<PolicyTarget, AppError> {
    let target = args
        .get("target")
        .ok_or_else(|| AppError::InvalidArgs("missing required argument 'target'".to_string()))?;
    serde_json::from_value(target.clone())
        .map_err(|e| AppError::InvalidArgs(format!("invalid target: {e}")))
}

fn parse_rules(args: &Value) -> Result<Vec<PolicyRule>, AppError> {
    let rule = args
        .get("rule")
        .ok_or_else(|| AppError::InvalidArgs("missing required argument 'rule'".to_string()))?;
    serde_json::from_value(rule.clone()).map_err(|e| AppError::InvalidArgs(format!("invalid rule: {e}")))
}

/// requirement 7: "All tenant-key only." -- every `host.policy.*`/
/// `host.audit.*` tool refuses a call that carries an end-user identity
/// (AC8), rather than silently scoping to that end user the way
/// `host.state.*`'s own `end_user` argument does.
fn require_tenant_key(end_user: Option<&EndUser>) -> Result<(), AppError> {
    if end_user.is_some() {
        return Err(AppError::Structured {
            code: "tenant_key_required",
            message: "this tool is tenant-key only and refuses a call carrying an end-user identity"
                .to_string(),
            data: json!({}),
        });
    }
    Ok(())
}

/// requirement 7: `host.policy.set(target, rule, replace?)`. Refuses
/// (`policy_widening`) a proposed rule that widens an existing `Literal`-
/// valued rule on the same target/column unless `replace: true` (AC10).
pub async fn policy_set(
    state: &AppState,
    tenant: &Tenant,
    args: &Value,
    end_user: Option<&EndUser>,
) -> Result<Value, AppError> {
    require_tenant_key(end_user)?;
    let target = parse_target(args)?;
    let rule = parse_rules(args)?;
    let replace = arg_bool(args, "replace");

    let existing = state
        .db
        .get_row_policy(tenant.id, target.kind().to_string(), target.value().to_string())
        .await?;

    if !replace
        && let Some(existing) = &existing
    {
        let existing_rule: Vec<PolicyRule> = serde_json::from_str(&existing.rule_json)
            .map_err(|e| AppError::Internal(format!("stored row policy rule_json is corrupt: {e}")))?;
        if let Some((old, new)) = policy::find_widening(&existing_rule, &rule) {
            return Err(AppError::Structured {
                code: "policy_widening",
                message: format!(
                    "proposed rule on '{}' widens the existing rule on column '{}' from {} to {} \
                     -- set replace: true to confirm",
                    target.value(),
                    old.column_or_attr,
                    describe_rule_value(&old.value),
                    describe_rule_value(&new.value)
                ),
                data: json!({"column_or_attr": old.column_or_attr, "target": target.value()}),
            });
        }
    }

    let version = existing.as_ref().map(|e| e.version + 1).unwrap_or(1);
    let rule_json = serde_json::to_string(&rule).map_err(|e| AppError::Internal(e.to_string()))?;
    let created_unix = now_unix();
    let record = state
        .db
        .upsert_row_policy(tenant.id, target.kind().to_string(), target.value().to_string(), version, rule_json, created_unix)
        .await?;

    Ok(json!({
        "target": target,
        "version": record.version,
        "rule": rule,
    }))
}

/// requirement 7: `host.policy.list()`.
pub async fn policy_list(state: &AppState, tenant: &Tenant, end_user: Option<&EndUser>) -> Result<Value, AppError> {
    require_tenant_key(end_user)?;
    let records = state.db.list_row_policies(tenant.id).await?;
    let policies: Vec<Value> = records
        .into_iter()
        .map(|r| {
            let rule: Value = serde_json::from_str(&r.rule_json).unwrap_or_else(|_| json!([]));
            let target = PolicyTarget::from_kind_value(&r.target_kind, r.target_value);
            json!({
                "target": target,
                "version": r.version,
                "rule": rule,
                "created_unix": r.created_unix,
            })
        })
        .collect();
    Ok(json!({"policies": policies}))
}

/// requirement 7: `host.policy.attrs_set(subject, attrs)` -- merges the
/// given attributes into `subject`'s stored set for this tenant.
pub async fn policy_attrs_set(
    state: &AppState,
    tenant: &Tenant,
    args: &Value,
    end_user: Option<&EndUser>,
) -> Result<Value, AppError> {
    require_tenant_key(end_user)?;
    let subject = arg_str(args, "subject")?;
    let attrs_value = args
        .get("attrs")
        .ok_or_else(|| AppError::InvalidArgs("missing required argument 'attrs'".to_string()))?;
    let attrs_obj = attrs_value
        .as_object()
        .ok_or_else(|| AppError::InvalidArgs("attrs must be an object".to_string()))?;
    let attrs: BTreeMap<String, Value> = attrs_obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    state.db.end_user_attrs_set(tenant.id, subject.clone(), attrs.clone()).await?;
    Ok(json!({"subject": subject, "attrs": attrs}))
}

fn arg_i64(args: &Value, name: &str) -> Result<i64, AppError> {
    args.get(name)
        .and_then(Value::as_i64)
        .ok_or_else(|| AppError::InvalidArgs(format!("missing required argument '{name}'")))
}

/// requirement 7: `host.audit.verify(from_id, to_id)` -- recomputes the
/// chain over `[from_id, to_id]` and reports whether it's intact, naming
/// the first broken id if not (AC7).
pub async fn audit_verify(
    state: &AppState,
    tenant: &Tenant,
    args: &Value,
    end_user: Option<&EndUser>,
) -> Result<Value, AppError> {
    require_tenant_key(end_user)?;
    let from_id = arg_i64(args, "from_id")?;
    let to_id = arg_i64(args, "to_id")?;
    let rows = state.db.audit_chain_range(tenant.id, from_id, to_id).await?;
    let records: Vec<audit::RetrievalAuditRecord> = rows.into_iter().map(Into::into).collect();
    let result = audit::verify_chain(&records);
    Ok(json!({"intact": result.intact, "first_broken_id": result.first_broken_id}))
}

/// requirement 7 (AC8): `host.audit.chain(subject?, limit?, before_id?)` --
/// newest-first page, tenant-key only. `withheld_count` is the number of
/// matching records beyond this page (`total - returned_count`), not a
/// per-record field.
pub async fn audit_chain(
    state: &AppState,
    tenant: &Tenant,
    args: &Value,
    end_user: Option<&EndUser>,
) -> Result<Value, AppError> {
    require_tenant_key(end_user)?;
    let subject = args.get("subject").and_then(Value::as_str).map(str::to_string);
    let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(50).clamp(1, 500);
    let before_id = args.get("before_id").and_then(Value::as_i64);
    let (records, total) = state.db.audit_chain_list(tenant.id, subject, limit, before_id).await?;
    let returned_count = records.len() as i64;
    let withheld_count = total - returned_count;
    let records_json: Vec<Value> = records
        .into_iter()
        .map(|r| {
            json!({
                "id": r.id,
                "subject": r.subject,
                "plane": r.plane,
                "policy_hash": r.policy_hash,
                "applied": r.applied,
                "returned_count": r.returned_count,
                "withheld_count": r.withheld_count,
                "timestamp": r.timestamp_unix,
                "request_id": r.request_id,
            })
        })
        .collect();
    Ok(json!({"records": records_json, "returned_count": returned_count, "withheld_count": withheld_count}))
}
