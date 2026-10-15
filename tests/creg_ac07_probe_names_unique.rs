//! AC7 (P1) -- PRD-mcphost-server-capabilities-from-registry. Every
//! `Capability.probe` is a name synthorg's probe table can hold
//! (`^[a-z_]+$`) and no two rows share one; a duplicate fails
//! `creg_ac07_probe_names_unique` naming both rows.

use mcphost::capabilities::{CAPABILITIES, Capability};

fn row(name: &'static str, probe: &'static str) -> Capability {
    Capability {
        name,
        methods: &["x/y"],
        implemented: false,
        list_changed: false,
        subscribe: false,
        ceiling: None,
        probe,
        alternative: None,
        help_code: "capability_unsupported",
    }
}

/// Every problem with the probe column of `rows`, each naming the rows.
fn probe_problems(rows: &[Capability]) -> Vec<String> {
    let mut problems = Vec::new();
    for (i, a) in rows.iter().enumerate() {
        if a.probe.is_empty() || !a.probe.bytes().all(|b| b.is_ascii_lowercase() || b == b'_') {
            problems.push(format!("row `{}` probe `{}` does not match ^[a-z_]+$", a.name, a.probe));
        }
        for b in &rows[i + 1..] {
            if a.probe == b.probe {
                problems.push(format!("rows `{}` and `{}` share probe name `{}`", a.name, b.name, a.probe));
            }
        }
    }
    problems
}

#[test]
fn creg_ac07_probe_names_unique() {
    let problems = probe_problems(CAPABILITIES);
    assert!(problems.is_empty(), "{}", problems.join("; "));
}

#[test]
fn duplicate_probe_names_are_reported_naming_both_rows() {
    let rows = [row("alpha", "same"), row("beta", "same"), row("gamma", "other")];
    let problems = probe_problems(&rows);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("`alpha`") && problems[0].contains("`beta`"), "{problems:?}");
    assert!(probe_problems(&[row("Bad", "Has-Dash")]).iter().any(|p| p.contains("^[a-z_]+$")));
}
