//! Policy model, `compile()`, and the dominance/widening check --
//! requirement 1/2/9. Ported (shape, not code -- the source tree named in
//! the PRD's grounding was not available on this build host) from
//! ai-stack's `policy.rs`/`gateway.rs` `EntitlementPolicy`/`CompiledPolicy`/
//! `dominates()`, renamed to this crate's `RowPolicy` shape; the source
//! crate's namespace/enrollment/backend_registry machinery is not carried
//! over (technical considerations: "Port scope").

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// requirement 1: `Table(name) | DocPrefix(prefix)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyTarget {
    Table(String),
    DocPrefix(String),
}

impl PolicyTarget {
    pub fn kind(&self) -> &'static str {
        match self {
            PolicyTarget::Table(_) => "table",
            PolicyTarget::DocPrefix(_) => "doc_prefix",
        }
    }

    pub fn value(&self) -> &str {
        match self {
            PolicyTarget::Table(v) | PolicyTarget::DocPrefix(v) => v,
        }
    }

    pub fn from_kind_value(kind: &str, value: String) -> Option<Self> {
        match kind {
            "table" => Some(PolicyTarget::Table(value)),
            "doc_prefix" => Some(PolicyTarget::DocPrefix(value)),
            _ => None,
        }
    }
}

/// requirement 1: `op: eq | in`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleOp {
    Eq,
    In,
}

impl RuleOp {
    pub fn as_str(self) -> &'static str {
        match self {
            RuleOp::Eq => "eq",
            RuleOp::In => "in",
        }
    }
}

/// requirement 1: `value: Literal | Attr(end_user_attr)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleValue {
    Literal(Value),
    Attr(String),
}

/// requirement 1: `{column_or_attr, op, value}`. For a [`PolicyTarget::Table`],
/// `column_or_attr` names the SQL column the compiled predicate filters on.
/// For a [`PolicyTarget::DocPrefix`], it names the end-user attribute
/// `compile()` checks the subject's own value of against `value` -- there
/// is no SQL column to filter, so the rule instead answers "is this
/// subject admitted".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PolicyRule {
    pub column_or_attr: String,
    pub op: RuleOp,
    pub value: RuleValue,
}

/// requirement 1: `RowPolicy {id, tenant_id, version, target, rule,
/// created_unix}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RowPolicy {
    pub id: i64,
    pub tenant_id: i64,
    pub version: i64,
    pub target: PolicyTarget,
    pub rule: Vec<PolicyRule>,
    pub created_unix: i64,
}

/// requirement 3: `SecurityContext {subject, issuer, method, attrs}`,
/// ported from ai-stack's `context.rs`.
#[derive(Debug, Clone, PartialEq)]
pub struct SecurityContext {
    pub subject: String,
    pub issuer: Option<String>,
    pub method: String,
    pub attrs: BTreeMap<String, Value>,
}

/// requirement 2: `CompiledPolicy {rls_clause, metadata_filter, policy_hash}`,
/// renamed to this crate's field names. `rls_predicate` uses `{p0}`,
/// `{p1}`, ... template placeholders positionally aligned with
/// `rls_params` -- never a literal value spliced into the string itself
/// (requirement 4: "string concatenation... is forbidden") -- a caller
/// binds them (AST placeholders for the SQL path, direct comparison for
/// the docs path).
#[derive(Debug, Clone, PartialEq)]
pub struct CompiledPolicy {
    pub rls_predicate: String,
    pub rls_params: Vec<Value>,
    pub docs_prefix: Option<String>,
    pub docs_subject_allowed: bool,
    pub policy_hash: String,
}

/// requirement 2: resolves one rule's allowed SQL-side values for `ctx`.
/// `Attr(name)` with no matching attribute on the subject resolves to the
/// empty set -- the fail-closed case AC2/AC6 both pin.
fn resolved_values(rule: &PolicyRule, ctx: &SecurityContext) -> Vec<Value> {
    match &rule.value {
        RuleValue::Literal(Value::Array(items)) => items.clone(),
        RuleValue::Literal(other) => vec![other.clone()],
        RuleValue::Attr(attr) => ctx.attrs.get(attr).cloned().into_iter().collect(),
    }
}

/// requirement 5 (AC5): does `ctx`'s own subject satisfy this rule --
/// answers "is this subject admitted", not "which rows".
fn subject_satisfies(rule: &PolicyRule, ctx: &SecurityContext) -> bool {
    let Some(subject_value) = ctx.attrs.get(&rule.column_or_attr) else {
        return false;
    };
    match &rule.value {
        RuleValue::Literal(Value::Array(items)) => items.contains(subject_value),
        RuleValue::Literal(other) => other == subject_value,
        RuleValue::Attr(attr) => ctx.attrs.get(attr) == Some(subject_value),
    }
}

/// requirement 2: `policy_hash = sha256(canonical JSON of policy version +
/// resolved rule values)`. `serde_json::Value::Object` is a `BTreeMap`
/// under the hood in this crate (no `preserve_order` feature is enabled on
/// `serde_json` -- see `Cargo.lock`), so `serde_json::to_vec` of any
/// `Value` built from `json!` already serializes with sorted keys; no
/// separate canonicalization pass is needed.
fn policy_hash(policy: &RowPolicy, resolved: &[(String, &'static str, Vec<Value>)]) -> String {
    let resolved_json: Vec<Value> = resolved
        .iter()
        .map(|(column, op, values)| {
            json!({
                "column_or_attr": column,
                "op": op,
                "resolved_value": values,
            })
        })
        .collect();
    let canonical = json!({
        "version": policy.version,
        "target_kind": policy.target.kind(),
        "target_value": policy.target.value(),
        "resolved": resolved_json,
    });
    let bytes = serde_json::to_vec(&canonical).expect("policy hash input is always serializable");
    crate::billing::sha256_hex(&bytes)
}

/// requirement 2 (AC1/AC2/AC6): `compile(policy, context) -> CompiledPolicy`.
/// `policy.rule` is empty for the synthesized "no policy configured for
/// this target" case (goal 3's fail-closed default): an empty rule list
/// resolves to an empty allowed set on the SQL side (`1 = 0`) and
/// `docs_subject_allowed: false` on the docs side, exactly as a real rule
/// with an empty resolved set would -- fail-closed and "has a policy but
/// admits nothing" share one code path.
pub fn compile(policy: &RowPolicy, ctx: &SecurityContext) -> CompiledPolicy {
    let resolved: Vec<(String, &'static str, Vec<Value>)> = policy
        .rule
        .iter()
        .map(|r| (r.column_or_attr.clone(), r.op.as_str(), resolved_values(r, ctx)))
        .collect();
    let policy_hash = policy_hash(policy, &resolved);

    match &policy.target {
        PolicyTarget::Table(_) => {
            let mut predicate_parts = Vec::new();
            let mut params: Vec<Value> = Vec::new();
            let mut fail_closed = policy.rule.is_empty();
            for (column, op, values) in &resolved {
                if values.is_empty() {
                    fail_closed = true;
                    break;
                }
                let quoted = column.replace('"', "\"\"");
                match *op {
                    "eq" => {
                        predicate_parts.push(format!("\"{quoted}\" = {{p{}}}", params.len()));
                        params.push(values[0].clone());
                    }
                    _ => {
                        let placeholders: Vec<String> = values
                            .iter()
                            .enumerate()
                            .map(|(i, _)| format!("{{p{}}}", params.len() + i))
                            .collect();
                        predicate_parts.push(format!("\"{quoted}\" IN ({})", placeholders.join(", ")));
                        params.extend(values.iter().cloned());
                    }
                }
            }
            if fail_closed {
                CompiledPolicy {
                    rls_predicate: "1 = 0".to_string(),
                    rls_params: Vec::new(),
                    docs_prefix: None,
                    docs_subject_allowed: false,
                    policy_hash,
                }
            } else {
                CompiledPolicy {
                    rls_predicate: predicate_parts.join(" AND "),
                    rls_params: params,
                    docs_prefix: None,
                    docs_subject_allowed: false,
                    policy_hash,
                }
            }
        }
        PolicyTarget::DocPrefix(prefix) => {
            let allowed = !policy.rule.is_empty() && policy.rule.iter().all(|r| subject_satisfies(r, ctx));
            CompiledPolicy {
                rls_predicate: "1 = 0".to_string(),
                rls_params: Vec::new(),
                docs_prefix: Some(prefix.clone()),
                docs_subject_allowed: allowed,
                policy_hash,
            }
        }
    }
}

fn as_value_set(v: &Value) -> BTreeSet<String> {
    match v {
        Value::Array(items) => items.iter().map(|i| i.to_string()).collect(),
        other => std::iter::once(other.to_string()).collect(),
    }
}

/// requirement 9 (AC10): ported from ai-stack's `gateway.rs` `dominates()`,
/// narrowed to this crate's literal-value widening check -- an
/// `Attr`-valued rule resolves per subject at query time rather than to a
/// fixed value set at `host.policy.set` time, so there is nothing to
/// compare there; only two `Literal`-valued rules on the same column can
/// "widen" in this sense.
pub fn widens(old_rule: &PolicyRule, new_rule: &PolicyRule) -> bool {
    if old_rule.column_or_attr != new_rule.column_or_attr {
        return false;
    }
    let (RuleValue::Literal(old_v), RuleValue::Literal(new_v)) = (&old_rule.value, &new_rule.value) else {
        return false;
    };
    let old_set = as_value_set(old_v);
    let new_set = as_value_set(new_v);
    new_set.is_superset(&old_set) && new_set != old_set
}

/// requirement 9: the first `(old, new)` rule pair where `new` widens
/// `old`, for `host.policy.set`'s refusal message.
pub fn find_widening<'a>(
    old_rules: &'a [PolicyRule],
    new_rules: &'a [PolicyRule],
) -> Option<(&'a PolicyRule, &'a PolicyRule)> {
    for old in old_rules {
        for new in new_rules {
            if widens(old, new) {
                return Some((old, new));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(attrs: &[(&str, Value)]) -> SecurityContext {
        SecurityContext {
            subject: "alice".to_string(),
            issuer: None,
            method: "oauth".to_string(),
            attrs: attrs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(),
        }
    }

    fn table_policy(rule: Vec<PolicyRule>) -> RowPolicy {
        RowPolicy {
            id: 1,
            tenant_id: 1,
            version: 1,
            target: PolicyTarget::Table("orders".to_string()),
            rule,
            created_unix: 0,
        }
    }

    /// requirement 2: "The port carries the source crate's determinism...
    /// tests" -- compiling the same policy against the same context twice
    /// yields byte-identical output (AC6).
    #[test]
    fn compile_is_deterministic() {
        let policy = table_policy(vec![PolicyRule {
            column_or_attr: "region".to_string(),
            op: RuleOp::Eq,
            value: RuleValue::Attr("region".to_string()),
        }]);
        let context = ctx(&[("region", json!("EU"))]);
        let a = compile(&policy, &context);
        let b = compile(&policy, &context);
        assert_eq!(a, b);
    }

    /// requirement 2: "...and empty-set tests" -- an `Attr` rule with no
    /// matching attribute compiles to `1 = 0` (AC2), and an empty `rule`
    /// list does too (AC6/goal 3).
    #[test]
    fn compile_empty_set_fails_closed() {
        let policy = table_policy(vec![PolicyRule {
            column_or_attr: "region".to_string(),
            op: RuleOp::Eq,
            value: RuleValue::Attr("region".to_string()),
        }]);
        let bob = ctx(&[]);
        let compiled = compile(&policy, &bob);
        assert_eq!(compiled.rls_predicate, "1 = 0");
        assert!(compiled.rls_params.is_empty());

        let no_policy = table_policy(vec![]);
        let compiled_no_policy = compile(&no_policy, &bob);
        assert_eq!(compiled_no_policy.rls_predicate, "1 = 0");
    }

    #[test]
    fn different_subjects_get_different_hashes() {
        let policy = table_policy(vec![PolicyRule {
            column_or_attr: "region".to_string(),
            op: RuleOp::Eq,
            value: RuleValue::Attr("region".to_string()),
        }]);
        let alice = ctx(&[("region", json!("EU"))]);
        let carol = ctx(&[("region", json!("US"))]);
        assert_ne!(compile(&policy, &alice).policy_hash, compile(&policy, &carol).policy_hash);
    }

    #[test]
    fn widens_detects_a_superset_but_not_a_disjoint_or_narrower_set() {
        let old = PolicyRule {
            column_or_attr: "region".to_string(),
            op: RuleOp::In,
            value: RuleValue::Literal(json!(["EU"])),
        };
        let wider = PolicyRule {
            column_or_attr: "region".to_string(),
            op: RuleOp::In,
            value: RuleValue::Literal(json!(["EU", "US"])),
        };
        let same = PolicyRule { value: RuleValue::Literal(json!(["EU"])), ..old.clone() };
        let narrower = PolicyRule { value: RuleValue::Literal(json!([])), ..old.clone() };
        assert!(widens(&old, &wider));
        assert!(!widens(&old, &same));
        assert!(!widens(&old, &narrower));
    }
}
