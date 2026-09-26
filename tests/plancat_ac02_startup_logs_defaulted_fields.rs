//! PRD-mcphost-plan-catalog-state-quota-defaults
//! AC2 (P0) — Given a `plans.toml` naming only price/tools/calls/secrets/
//! description for `free` and `pro`, When the host starts, Then the log
//! contains `plans.toml: plan=free defaulted=[` naming `state_bytes_max`
//! and `plans.toml: plan=pro defaulted=[`.

use crate::busyaudit;

use busyaudit::{capture_tracing, scratch_data_dir};
use mcphost::plans::PlanCatalog;

const PLANS_TOML: &str = "\
[[plan]]
name = \"free\"
price_usd_month = 0
tools_max = 50
calls_per_day = 500
secrets_max = 2
description = \"Free: 50 tools, 500 calls/day, 2 secrets. No card required.\"

[[plan]]
name = \"pro\"
price_usd_month = 19
tools_max = 50
calls_per_day = 100000
secrets_max = 20
description = \"Pro tier\"
";

#[test]
fn startup_logs_one_defaulted_line_per_plan_naming_state_bytes_max() {
    let dir = scratch_data_dir("plancat-ac02");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("plans.toml");
    std::fs::write(&path, PLANS_TOML).unwrap();

    let (result, log) = capture_tracing(|| PlanCatalog::load_or_init(&path));
    result.expect("load_or_init");

    assert!(
        log.contains("plans.toml: plan=free defaulted=["),
        "expected a free defaulted= line in:\n{log}"
    );
    let free_line = log
        .lines()
        .find(|l| l.contains("plans.toml: plan=free defaulted=["))
        .unwrap();
    assert!(
        free_line.contains("state_bytes_max"),
        "free's defaulted= line must name state_bytes_max: {free_line}"
    );
    assert!(
        log.contains("plans.toml: plan=pro defaulted=["),
        "expected a pro defaulted= line in:\n{log}"
    );

    std::fs::remove_dir_all(&dir).ok();
}
