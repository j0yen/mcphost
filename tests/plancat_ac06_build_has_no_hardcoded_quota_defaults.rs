//! PRD-mcphost-plan-catalog-state-quota-defaults
//! AC6 (P0) — Given `src/plans.rs` after this PRD, When grepped, Then no
//! quota field in `PlanBuilder::build` uses `unwrap_or(0)` or a numeric
//! literal (test reads the source file).

/// Extracts `fn build` through its matching closing brace by counting `{`
/// / `}` from the `fn build` line onward -- `PlanBuilder::build` is the
/// only function of that name in this file, so a plain substring search
/// for the header is enough to find the start.
fn build_fn_body(source: &str) -> &str {
    let start = source.find("fn build(").expect("PlanBuilder::build must exist");
    let after_header = &source[start..];
    let open = after_header.find('{').expect("fn build must have a body");
    let mut depth = 0i32;
    for (i, ch) in after_header[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &after_header[..open + i + 1];
                }
            }
            _ => {}
        }
    }
    panic!("unbalanced braces in PlanBuilder::build");
}

#[test]
fn build_defaults_every_quota_from_the_catalog_not_a_literal() {
    let source = std::fs::read_to_string("src/plans.rs").expect("read src/plans.rs");
    let body = build_fn_body(&source);

    assert!(
        !body.contains("unwrap_or(0)"),
        "PlanBuilder::build must not default any quota with unwrap_or(0):\n{body}"
    );

    // Every `.unwrap_or(...)` in the body must fall back to a field read
    // off `defaults` (e.g. `defaults.state_bytes_max`), never a bare
    // numeric literal.
    for (i, _) in body.match_indices(".unwrap_or(") {
        let after = &body[i + ".unwrap_or(".len()..];
        assert!(
            after.starts_with("defaults."),
            "PlanBuilder::build's unwrap_or(...) must fall back to a defaults.<field> lookup, not a literal, at: ...{}",
            &body[i..(i + 60).min(body.len())]
        );
    }
}
