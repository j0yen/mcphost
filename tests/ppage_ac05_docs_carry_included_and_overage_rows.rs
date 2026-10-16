//! PRD-mcphost-pricing-page-from-plan-table AC5 (P0) -- Given
//! `docs/plans.md` and `www/llms-full.txt`, When `gen-docs` and
//! `gen-llms-full.sh` run, Then both carry `calls_included_month` and
//! `overage_usd_per_1000_calls` rows and the committed files equal the
//! outputs. (`gen-llms-full.sh --check` is the CI half for llms-full; here
//! its plans section is compared with the fresh render.)

use mcphost::gendocs::render_plans_markdown;
use mcphost::plans::PlanCatalog;

const FIELDS: [&str; 2] = ["calls_included_month", "overage_usd_per_1000_calls"];

#[test]
fn docs_plans_md_equals_the_render_and_carries_both_rows() {
    let committed = include_str!("../docs/plans.md");
    let fresh = render_plans_markdown(&PlanCatalog::default_catalog());
    assert_eq!(committed, fresh, "docs/plans.md has drifted -- run `mcphost gen-docs`");
    for field in FIELDS {
        let row = committed.lines().find(|l| l.starts_with(&format!("| `{field}` |")));
        assert!(row.is_some(), "docs/plans.md lacks a `{field}` row");
    }
    assert!(committed.contains("| `calls_included_month` | 0 | 50000 |"));
    assert!(committed.contains("| `overage_usd_per_1000_calls` | 0 | 1 |"));
}

#[test]
fn llms_full_embeds_the_fresh_plans_render_with_both_rows() {
    let llms_full = include_str!("../www/llms-full.txt");
    let fresh = render_plans_markdown(&PlanCatalog::default_catalog());
    assert!(
        llms_full.contains(fresh.trim_end()),
        "www/llms-full.txt does not embed the fresh docs/plans.md render -- run scripts/gen-llms-full.sh"
    );
    for field in FIELDS {
        assert!(
            llms_full.lines().any(|l| l.starts_with(&format!("| `{field}` |"))),
            "www/llms-full.txt lacks a `{field}` row"
        );
    }
}
