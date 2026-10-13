//! AC1 (P0) -- Given a free-plan tenant with 5 drafts today and two
//! concurrent sixth calls, When both run, Then both are refused with
//! `plan_limit` naming `drafts_per_day: 5` and `draft_events` holds exactly 5
//! rows for the day.

use crate::common;
use common::{McpClient, TestServer, python_kind_registry, signup};
use mcphost::db::DraftReservation;
use mcphost::drafter::{Draft, FakeDrafter};
use mcphost::plans::PlanCatalog;
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn two_concurrent_sixth_drafts_are_both_plan_limit_and_leave_five_rows() {
    let limit = PlanCatalog::default_catalog().get("free").expect("free plan").drafts_per_day;
    assert_eq!(limit, 5, "free plan drafts_per_day");

    // The fake has drafts queued: if the budget check let a call through, it
    // would consume one and the row count would exceed five.
    let spare = Draft { spec: json!({"source": "def main(args):\n    return {}\n"}), ..Draft::default() };
    let fake = Arc::new(FakeDrafter::new(vec![spare.clone(), spare]));
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_drafter(python_kind_registry(&envs_dir.0), fake.clone()).await;
    let (ns, key) = signup(&server.base_url, "TFDB AC1 Tenant").await;
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns.clone())
        .await
        .expect("lookup")
        .expect("tenant");

    let day_start = mcphost::state::now_unix().rem_euclid(86_400);
    let day_start = mcphost::state::now_unix() - day_start;
    for _ in 0..limit {
        let r = server.state.db.reserve_draft(tenant.id, day_start, limit, 200).await.expect("reserve");
        assert!(matches!(r, DraftReservation::Reserved(_)), "{r:?}");
    }

    let a = McpClient::with_bearer(&server.base_url, &key);
    let b = McpClient::with_bearer(&server.base_url, &key);
    let args = json!({"description": "return the current UTC time as ISO 8601"});
    let (ra, rb) = tokio::join!(
        a.tools_call("host.tool_from_description", args.clone()),
        b.tools_call("host.tool_from_description", args.clone()),
    );
    for r in [ra, rb] {
        let e = r.expect_err("sixth draft must be refused");
        assert_eq!(e.error_code.as_deref(), Some("plan_limit"), "{} {}", e.code, e.message);
        assert_eq!(e.data["drafts_per_day"], json!(5), "{}", e.data);
        assert!(e.message.contains("drafts_per_day"), "{}", e.message);
    }

    let drafts = server.state.db.drafts_since(day_start).await.expect("drafts_since");
    assert_eq!(drafts["n"], json!(5), "draft_events rows for the day: {drafts}");
    assert!(fake.prompts().is_empty(), "no model call may happen past the limit");
}
