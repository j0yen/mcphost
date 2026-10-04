//! PRD-mcphost-row-policy
//! AC6 (P0) — Given an empty allowed set, When compiled, Then the SQL
//! predicate is `1 = 0` and the docs filter admits nothing, and compiling
//! twice yields the same hash.
//!
//! Uses an explicit empty `Literal` set (`op: "in"`, `value: []`) rather
//! than an `Attr` the subject simply lacks (that's AC2's case) -- the
//! subject here *has* a `region`/`role` attribute, but the configured
//! allowed set is empty, so this pins the empty-set branch on its own.

use mcphost::rowpolicy::policy::compile;
use mcphost::rowpolicy::{PolicyRule, PolicyTarget, RowPolicy, RuleOp, RuleValue, SecurityContext};
use serde_json::json;
use std::collections::BTreeMap;

fn ctx() -> SecurityContext {
    let mut attrs = BTreeMap::new();
    attrs.insert("region".to_string(), json!("EU"));
    attrs.insert("role".to_string(), json!("hr"));
    SecurityContext { subject: "alice".to_string(), issuer: None, method: "assertion".to_string(), attrs }
}

#[test]
fn empty_allowed_set_is_1_equals_0_for_sql_and_deterministic() {
    let policy = RowPolicy {
        id: 1,
        tenant_id: 1,
        version: 1,
        target: PolicyTarget::Table("orders".to_string()),
        rule: vec![PolicyRule {
            column_or_attr: "region".to_string(),
            op: RuleOp::In,
            value: RuleValue::Literal(json!([])),
        }],
        created_unix: 0,
    };

    let first = compile(&policy, &ctx());
    let second = compile(&policy, &ctx());

    assert_eq!(first.rls_predicate, "1 = 0", "an empty allowed set must compile to 1 = 0");
    assert!(first.rls_params.is_empty(), "1 = 0 needs no bound parameters");
    assert_eq!(first.policy_hash, second.policy_hash, "compiling twice must yield the same hash");
}

#[test]
fn empty_allowed_set_denies_every_subject_for_docs() {
    let policy = RowPolicy {
        id: 2,
        tenant_id: 1,
        version: 1,
        target: PolicyTarget::DocPrefix("hr/".to_string()),
        rule: vec![PolicyRule {
            column_or_attr: "role".to_string(),
            op: RuleOp::In,
            value: RuleValue::Literal(json!([])),
        }],
        created_unix: 0,
    };

    let first = compile(&policy, &ctx());
    let second = compile(&policy, &ctx());

    assert!(
        !first.docs_subject_allowed,
        "an empty allowed set must admit no subject, even one that otherwise has the attribute"
    );
    assert_eq!(first.docs_prefix.as_deref(), Some("hr/"));
    assert_eq!(first.policy_hash, second.policy_hash, "compiling twice must yield the same hash");
}
