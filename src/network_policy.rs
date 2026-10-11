//! PRD-mcphost-sandbox-egress-allowlist: the one place that knows a spec's
//! `network` field needs the `pro` plan -- shared by `control::tool_publish`
//! (refuses before any tool row is written, AC1/AC2) and
//! `kinds::python::PythonKind::network_mode` (refuses before any sandbox is
//! spawned, AC3/AC6), so the two can never drift on which spelling is gated
//! the way `src/control.rs:724` used to (only `"egress"`, letting
//! `"public"` -- the exact same sandbox grant, `kinds::python`'s own
//! `network_mode` -- through unchecked).

use serde_json::{Value, json};

/// PRD-mcphost-plan-limits-generated: the plan `network: "public"`/`"egress"`
/// needs -- the one place that names it (publish gate, run gate and the
/// generated schema/quickstart text all read this).
pub const EGRESS_PLAN: &str = "pro";

/// Every accepted python `network` value with the plan that unlocks it.
/// [`wants_egress`] is true exactly for the values gated on
/// [`EGRESS_PLAN`] (asserted in this module's tests).
pub const NETWORK_VALUES: &[(&str, &str)] = &[
    ("none", "free"),
    ("public", EGRESS_PLAN),
    ("egress", EGRESS_PLAN),
];

/// The `network` field description with the plan-gated values listed,
/// rendered from [`NETWORK_VALUES`].
pub fn network_field_description() -> &'static str {
    static TEXT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    TEXT.get_or_init(|| {
        let values: Vec<String> = NETWORK_VALUES
            .iter()
            .map(|(value, plan)| {
                if *plan == "free" {
                    format!("\"{value}\" (every plan)")
                } else {
                    format!("\"{value}\" (requires the {plan} plan)")
                }
            })
            .collect();
        format!(
            "{} -- a tool's own code reaches this tenant's data with `import mcphost` \
             (mcphost.table, mcphost.state, mcphost.docs, mcphost.lineage) even when \
             network is \"none\"",
            values.join(", ")
        )
    })
}

/// `"public"` and `"egress"` have always meant the same sandbox grant; only
/// the plan gate used to see one spelling. `None`/anything else (including
/// absent, i.e. the default) is not egress.
pub fn wants_egress(network: Option<&str>) -> bool {
    matches!(network, Some("public") | Some("egress"))
}

/// requirement 1/3: the `message`/`data` pair for a `network` value that
/// needs `plan` on a tenant that doesn't have it -- requirement 3's
/// "republish with network: none, or upgrade" hint, shared verbatim by
/// `AppError::plan_required` (publish path, `errors.rs`) and
/// `KindError::structured_with("plan_required", ...)` (run path,
/// `kinds::python`).
pub fn plan_required_fields(field: &str, plan: &str) -> (String, Value) {
    let mut data = json!({"plan": plan, "field": field});
    // PRD-mcphost-uptime-probe-recipe-green requirement 4: the most common
    // reason to ask for egress is watching URLs, which `host.uptime.create`
    // does on the free plan -- name it where the refusal happens.
    if field == "network" {
        data["alternative"] = json!(UPTIME_ALTERNATIVE);
        data["docs"] = json!("/llms.txt#uptime-probes-with-no-server");
    }
    (
        format!(
            "{field}: requires the {plan} plan -- republish with {field}: \"none\", or upgrade"
        ),
        data,
    )
}

/// The free-plan path to watching URLs, named by every refused `network`
/// request (`data.alternative`).
pub const UPTIME_ALTERNATIVE: &str = "host.uptime.create";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_and_egress_both_want_egress() {
        assert!(wants_egress(Some("public")));
        assert!(wants_egress(Some("egress")));
        assert!(!wants_egress(Some("none")));
        assert!(!wants_egress(None));
    }

    #[test]
    fn network_values_table_agrees_with_wants_egress() {
        for (value, plan) in NETWORK_VALUES {
            assert_eq!(wants_egress(Some(value)), *plan == EGRESS_PLAN, "{value}");
        }
    }
}
