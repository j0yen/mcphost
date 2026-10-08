//! AC4 (PRD-mcphost-plan-limits-generated) — Given `scripts/gen-llms-full.sh`
//! and `gen-docs`, When run, Then the ceilings table appears in
//! `www/llms-full.txt` and `docs/`, and the committed files equal the
//! outputs.
//!
//! `gen-docs`'s output is `gendocs::render_plans_markdown` (compared with the
//! committed `docs/plans.md`); `gen-llms-full.sh --check` proves the committed
//! `www/llms-full.txt` equals the script's output. The expected table rows are
//! derived from the plan struct, not typed here.

use mcphost::gendocs::render_plans_markdown;
use mcphost::plans::PlanCatalog;
use std::process::Command;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn ceilings_table_is_in_docs_and_llms_full_and_files_match_generators() {
    let catalog = PlanCatalog::default_catalog();
    let committed = std::fs::read_to_string(format!("{ROOT}/docs/plans.md")).expect("docs/plans.md");
    assert_eq!(committed, render_plans_markdown(&catalog), "docs/plans.md drifted; run `mcphost gen-docs`");

    let llms_full = std::fs::read_to_string(format!("{ROOT}/www/llms-full.txt")).expect("llms-full");
    assert!(llms_full.contains("## Plan ceilings"));
    for plan in &catalog.plans {
        for (name, value) in plan.ceilings_flat() {
            let row = format!("| `{name}` |");
            assert!(committed.contains(&row), "docs/plans.md lacks {row}");
            assert!(llms_full.contains(&row), "www/llms-full.txt lacks {row}");
            assert!(
                llms_full.lines().any(|l| l.starts_with(&row) && l.contains(&format!(" {value} |"))),
                "no row for {name} carries {value} ({})",
                plan.name
            );
        }
    }
    assert!(
        llms_full.contains("`budget.max_tool_latency_ms` | 30000 | 120000 |"),
        "the latency row must render both plans"
    );

    let status = Command::new("bash")
        .arg(format!("{ROOT}/scripts/gen-llms-full.sh"))
        .arg("--check")
        .current_dir(ROOT)
        .status()
        .expect("run gen-llms-full.sh --check");
    assert!(status.success(), "www/llms-full.txt is stale versus gen-llms-full.sh");
}
