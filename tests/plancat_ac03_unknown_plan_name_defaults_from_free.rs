//! PRD-mcphost-plan-catalog-state-quota-defaults
//! AC3 (P0) — Given a plan named `team` naming only `tools_max`, When it
//! loads, Then its unnamed quotas equal the `free` plan's defaults and the
//! log line names plan `team`.

use crate::busyaudit;

use busyaudit::{capture_tracing, scratch_data_dir};
use mcphost::plans::PlanCatalog;

const PLANS_TOML: &str = "\
[[plan]]
name = \"team\"
tools_max = 7
";

#[test]
fn unrecognized_plan_name_defaults_every_unnamed_quota_from_free() {
    let dir = scratch_data_dir("plancat-ac03");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("plans.toml");
    std::fs::write(&path, PLANS_TOML).unwrap();

    let (result, log) = capture_tracing(|| PlanCatalog::load_or_init(&path));
    let catalog = result.expect("load_or_init");

    let team = catalog.get("team").expect("team plan");
    assert_eq!(team.tools_max, 7, "the one named field stays as written");

    let free_defaults = PlanCatalog::default_catalog()
        .get("free")
        .expect("free defaults")
        .clone();
    assert_eq!(team.price_usd_month, free_defaults.price_usd_month);
    assert_eq!(team.calls_per_day, free_defaults.calls_per_day);
    assert_eq!(team.secrets_max, free_defaults.secrets_max);
    assert_eq!(team.state_bytes_max, free_defaults.state_bytes_max);
    assert_eq!(team.state_rows_max, free_defaults.state_rows_max);
    assert_eq!(team.table_bytes_max, free_defaults.table_bytes_max);
    assert_eq!(team.docs_bytes_max, free_defaults.docs_bytes_max);
    assert_eq!(team.jobs_concurrent, free_defaults.jobs_concurrent);
    assert_eq!(team.end_users_max, free_defaults.end_users_max);

    assert!(
        log.contains("plans.toml: plan=team defaulted=["),
        "expected a team defaulted= line in:\n{log}"
    );

    std::fs::remove_dir_all(&dir).ok();
}
