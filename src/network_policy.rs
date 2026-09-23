//! PRD-mcphost-sandbox-egress-allowlist: the one place that knows a spec's
//! `network` field needs the `pro` plan -- shared by `control::tool_publish`
//! (refuses before any tool row is written, AC1/AC2) and
//! `kinds::python::PythonKind::network_mode` (refuses before any sandbox is
//! spawned, AC3/AC6), so the two can never drift on which spelling is gated
//! the way `src/control.rs:724` used to (only `"egress"`, letting
//! `"public"` -- the exact same sandbox grant, `kinds::python`'s own
//! `network_mode` -- through unchecked).

use serde_json::{Value, json};

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
    (
        format!(
            "{field}: requires the {plan} plan -- republish with {field}: \"none\", or upgrade"
        ),
        json!({"plan": plan, "field": field}),
    )
}

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
}
