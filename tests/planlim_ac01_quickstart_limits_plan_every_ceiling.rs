//! AC1 (PRD-mcphost-plan-limits-generated) — Given `host.quickstart` on a
//! free-plan tenant, When `limits.plan` is read, Then it contains every
//! numeric ceiling field of the plan struct including
//! `budget.max_tool_latency_ms = 30000`; the expected keys are derived from
//! the struct's serialisation, not a literal list.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::plans::{BUDGET_FIELD_PREFIX, NON_CEILING_FIELDS, PlanCatalog};
use serde_json::{Value, json};

#[tokio::test]
async fn quickstart_limits_plan_carries_every_numeric_plan_field() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "PlanLim AC1").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let result = client
        .tools_call("host.quickstart", json!({"kind": "echo"}))
        .await
        .expect("host.quickstart ok");
    let structured = common::extract_structured(&result);
    let plan_limits = &structured["limits"]["plan"];

    let catalog = PlanCatalog::default_catalog();
    let free = catalog.get("free").expect("free plan");
    let Value::Object(fields) = serde_json::to_value(free).unwrap() else {
        panic!("Plan serialises to an object");
    };
    let mut checked = 0;
    for (key, value) in fields {
        if !value.is_number() || NON_CEILING_FIELDS.contains(&key.as_str()) {
            continue;
        }
        let got = match key.strip_prefix(BUDGET_FIELD_PREFIX) {
            Some(rest) => &plan_limits["budget"][rest],
            None => &plan_limits[key.as_str()],
        };
        assert_eq!(got, &value, "limits.plan is missing or wrong for {key}");
        checked += 1;
    }
    assert!(checked > 30, "expected the whole struct, checked {checked}");
    assert_eq!(plan_limits["budget"]["max_tool_latency_ms"], json!(30000));
    assert_eq!(plan_limits["name"], json!("free"));
    assert_eq!(plan_limits["network_public"], json!("pro"));
}
