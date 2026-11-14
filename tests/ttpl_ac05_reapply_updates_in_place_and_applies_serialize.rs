//! PRD-mcphost-tenant-templates
//! AC5 -- Given an applied template, When applied again with the same name,
//! Then no duplicate tool, table or trigger exists, `updated` lists the
//! objects, and the tool's version advanced by one; Given two concurrent
//! applies, Then one succeeds and one returns `conflict` with
//! `data.retry_after_s`.

use crate::ttpl;
use serde_json::json;

#[tokio::test]
async fn ttpl_ac05_reapply_updates_in_place_with_one_new_tool_version() {
    let fx = ttpl::start().await;
    let tenant = fx.tenant().await;
    let version = || async { fx.server.state.db.get_tool(tenant.id, "scheduled_job".into()).await.expect("get").expect("tool").current_version };

    let first = fx.ok("host.template_apply", json!({"name": "scheduled-job"})).await;
    assert_eq!(first["created"]["tools"], json!(["scheduled_job"]), "{first}");
    let after_first = fx.inventory().await;
    let v1 = version().await;

    let again = fx.ok("host.template_apply", json!({"name": "scheduled-job"})).await;
    assert_eq!(again["created"]["tools"], json!([]), "{again}");
    assert_eq!(again["created"]["triggers"], json!([]), "{again}");
    assert_eq!(again["updated"]["tools"], json!(["scheduled_job"]), "{again}");
    assert_eq!(again["updated"]["triggers"], first["created"]["triggers"], "the same trigger, updated in place: {again}");

    assert_eq!(fx.inventory().await, after_first, "no duplicate tool, table or trigger");
    assert_eq!(version().await, v1 + 1, "the tool's version advanced by one");
}

#[tokio::test]
async fn ttpl_ac05_a_second_apply_while_one_runs_gets_conflict_with_retry_after() {
    let fx = ttpl::start().await;

    // The first apply is in flight: it holds the tenant's lock for its whole run.
    let in_flight = mcphost::templates::lock_tenant(&fx.namespace).expect("the first apply takes the lock");
    let err = fx.apply("scheduled-job").await.expect_err("the second apply is refused");
    assert_eq!(err.error_code.as_deref(), Some("conflict"), "{err:?}");
    assert!(err.data["retry_after_s"].as_u64().is_some_and(|s| s > 0), "{err:?}");
    assert_eq!(fx.inventory().await.0, Vec::<String>::new(), "the refused apply created nothing");

    drop(in_flight);
    fx.ok("host.template_apply", json!({"name": "scheduled-job"})).await;
}

#[tokio::test]
async fn ttpl_ac05_concurrent_applies_each_succeed_or_get_conflict_and_never_duplicate() {
    let fx = ttpl::start().await;
    let (a, b) = tokio::join!(fx.apply("scheduled-job"), fx.apply("scheduled-job"));
    let results = [a, b];
    assert!(results.iter().any(Result::is_ok), "one apply succeeds");
    for r in &results {
        if let Err(e) = r {
            assert_eq!(e.error_code.as_deref(), Some("conflict"), "{e:?}");
            assert!(e.data["retry_after_s"].is_number(), "{e:?}");
        }
    }
    let (tools, triggers, _) = fx.inventory().await;
    assert_eq!((tools.len(), triggers.len()), (1, 1), "one tool and one trigger, never a duplicate");
}
