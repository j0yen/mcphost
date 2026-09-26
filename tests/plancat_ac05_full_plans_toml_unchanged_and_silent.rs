//! PRD-mcphost-plan-catalog-state-quota-defaults
//! AC5 (P0) — Given a `plans.toml` produced by `to_toml()` of the default
//! catalog, When it loads, Then the catalog is byte-for-byte equal to
//! `default_catalog()` and no `defaulted=` line is logged.

use crate::busyaudit;

use busyaudit::{capture_tracing, scratch_data_dir};
use mcphost::plans::PlanCatalog;

#[test]
fn a_fully_named_plans_toml_loads_unchanged_and_logs_nothing() {
    let dir = scratch_data_dir("plancat-ac05");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("plans.toml");
    let defaults = PlanCatalog::default_catalog();
    std::fs::write(&path, defaults.to_toml()).unwrap();

    let (result, log) = capture_tracing(|| PlanCatalog::load_or_init(&path));
    let catalog = result.expect("load_or_init");

    assert_eq!(catalog, defaults, "a fully-named file must load byte-for-byte identical");
    assert!(
        !log.contains("defaulted="),
        "a fully-named file must not log any defaulted= line: {log}"
    );

    std::fs::remove_dir_all(&dir).ok();
}
