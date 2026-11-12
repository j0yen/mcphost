//! PRD-mcphost-response-size-budget-explicit-v2
//! AC3 (P0) -- Given the free and pro plans, When `host.quickstart` runs,
//! Then `limits.plan.response_budget_bytes` equals the plan struct's field,
//! `limits.output_bytes_max` still reads the 1 MiB wall, and
//! `render_ceilings_table` lists the field in `docs/plans.md` with the docs
//! drift test green.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup, signup_and_make_pro};
use mcphost::plans::PlanCatalog;
use serde_json::json;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[tokio::test]
async fn rszb_ac03_quickstart_reports_the_plan_budget_and_keeps_the_wall() {
    let server = TestServer::start().await;
    let catalog = PlanCatalog::default_catalog();
    let (_ns, free_key) = signup(&server.base_url, "Rszb AC3 Free").await;
    let (_ns, pro_key, _id) = signup_and_make_pro(&server, "Rszb AC3 Pro", "cus_rszb_ac3").await;

    for (plan, key) in [("free", free_key), ("pro", pro_key)] {
        let client = McpClient::with_bearer(&server.base_url, &key);
        let quickstart = extract_structured(&client.tools_call("host.quickstart", json!({"kind": "echo"})).await.expect("quickstart"));
        let field = catalog.get(plan).expect("plan").response_budget_bytes;
        assert_eq!(quickstart["limits"]["plan"]["response_budget_bytes"], json!(field), "{plan}: {quickstart}");
        assert_eq!(quickstart["limits"]["plan"]["name"], json!(plan), "{quickstart}");
        assert_eq!(quickstart["limits"]["output_bytes_max"], json!(1024 * 1024), "{plan}: the kinds' wall is unchanged");
    }
    assert_eq!(catalog.get("free").expect("free").response_budget_bytes, 262_144);
    assert_eq!(catalog.get("pro").expect("pro").response_budget_bytes, 1_048_576);
}

#[test]
fn rszb_ac03_a_plans_toml_without_the_key_takes_its_own_plans_default() {
    let toml = "\
[[plan]]
name = \"free\"
price_usd_month = 0
tools_max = 50
calls_per_day = 500
secrets_max = 2
description = \"Free.\"

[[plan]]
name = \"pro\"
price_usd_month = 19
tools_max = 50
calls_per_day = 100000
secrets_max = 20
description = \"Pro.\"

[[plan]]
name = \"gold\"
price_usd_month = 99
tools_max = 50
calls_per_day = 100000
secrets_max = 20
description = \"An unknown name.\"
";
    let (catalog, defaulted) = PlanCatalog::from_toml(toml).expect("parse");
    assert_eq!(catalog.get("pro").expect("pro").response_budget_bytes, 1_048_576);
    assert_eq!(catalog.get("free").expect("free").response_budget_bytes, 262_144);
    assert_eq!(catalog.get("gold").expect("gold").response_budget_bytes, 262_144, "an unknown name takes the free default");
    let _ = defaulted;

    let with_key = toml.replacen("secrets_max = 2\n", "secrets_max = 2\nresponse_budget_bytes = 4096\n", 1);
    let (catalog, _) = PlanCatalog::from_toml(&with_key).expect("parse");
    assert_eq!(catalog.get("free").expect("free").response_budget_bytes, 4096);
}

#[test]
fn rszb_ac03_the_ceilings_table_lists_the_field_and_docs_plans_md_matches_it() {
    let catalog = PlanCatalog::default_catalog();
    let table = mcphost::gendocs::render_ceilings_table(&catalog);
    let row = "| `response_budget_bytes` | 262144 | 1048576 |";
    assert!(table.contains(row), "render_ceilings_table lacks {row}:\n{table}");

    let committed = std::fs::read_to_string(format!("{ROOT}/docs/plans.md")).expect("docs/plans.md");
    assert!(committed.contains(row), "docs/plans.md lacks {row}");
    assert_eq!(committed, mcphost::gendocs::render_plans_markdown(&catalog), "docs/plans.md drifted; run `mcphost gen-docs`");
}
