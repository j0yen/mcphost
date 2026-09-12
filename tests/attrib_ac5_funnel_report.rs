//! PRD-mcphost-tenant-attribution
//! AC5 — Given the tables as they exist, When `mcphost funnel --json`
//! runs, Then it reports the six stages for real and synthetic tenants
//! and the median signup→first-successful-call time, in under 2s on the
//! current DB.
//!
//! Exercises `mcphost::funnel::compute` directly (the exact function
//! `main.rs`'s `Funnel` subcommand calls) against a small, deterministic
//! scenario -- `Db::create_tenant_attributed`/`upsert_tool`/`record_call`
//! rather than the CLI's own process spawn, so this is a fast, in-process
//! assertion of the report's correctness rather than a shell-out smoke
//! test.

use crate::common;
use common::TempDataDir;
use mcphost::auth::{generate_key, generate_namespace, hash_key};
use mcphost::db::Db;
use mcphost::plans::PlanCatalog;
use serde_json::json;
use std::time::Instant;

#[tokio::test]
async fn funnel_reports_six_stages_split_real_and_synthetic() {
    let dir = TempDataDir::new();
    let db = Db::open(&dir.0).expect("open db");
    db.migrate().await.expect("migrate");
    let plans = PlanCatalog::default_catalog();

    // Real tenant A: signs up, publishes a tool, makes one successful call.
    let key_a = generate_key();
    let ns_a = generate_namespace();
    let tenant_a = db
        .create_tenant_attributed(
            "Real Caller A".to_string(),
            ns_a,
            hash_key(&key_a),
            None,
            Some("external".to_string()),
            Some("claude-code".to_string()),
            Some("2.1".to_string()),
            "external".to_string(),
            None,
        )
        .await
        .expect("create tenant a");
    db.upsert_tool(tenant_a.id, "greet".to_string(), "echo".to_string(), json!({}))
        .await
        .expect("publish tool");
    db.record_call(
        tenant_a.id,
        "greet".to_string(),
        12,
        true,
        None,
        None,
        None,
        "ok",
        tenant_a.origin.clone(),
        tenant_a.origin_detail.clone(),
    )
    .await
    .expect("record call");

    // Real tenant C: signs up and gets upgraded to `pro` -- never
    // publishes or calls.
    let key_c = generate_key();
    let ns_c = generate_namespace();
    let tenant_c = db
        .create_tenant_attributed(
            "Real Payer C".to_string(),
            ns_c,
            hash_key(&key_c),
            None,
            Some("external".to_string()),
            None,
            None,
            "external".to_string(),
            None,
        )
        .await
        .expect("create tenant c");
    db.upgrade_tenant_plan(tenant_c.id, "pro".to_string(), "unix:0.0".to_string(), None)
        .await
        .expect("upgrade tenant c");

    // Synthetic tenant B: a plain loopback signup, no activity.
    let key_b = generate_key();
    let ns_b = generate_namespace();
    db.create_tenant_attributed(
        "panel_persona_b".to_string(),
        ns_b,
        hash_key(&key_b),
        Some("harness:unstamped".to_string()),
        Some("loopback".to_string()),
        None,
        None,
        "synthetic".to_string(),
        None,
    )
    .await
    .expect("create tenant b");

    let started = Instant::now();
    let report = mcphost::funnel::compute(&db, &plans, None)
        .await
        .expect("compute funnel");
    assert!(
        started.elapsed().as_secs() < 2,
        "AC5: funnel must compute in under 2s"
    );

    assert_eq!(report.real.signed_up, 2, "{report:?}");
    assert_eq!(report.real.published, 1, "{report:?}");
    assert_eq!(report.real.called, 1, "{report:?}");
    assert_eq!(report.real.upgraded, 1, "{report:?}");
    assert!(
        report.real.median_signup_to_first_call_secs.is_some(),
        "{report:?}"
    );

    assert_eq!(report.synthetic.signed_up, 1, "{report:?}");
    assert_eq!(report.synthetic.published, 0, "{report:?}");
    assert_eq!(report.synthetic.called, 0, "{report:?}");
    assert_eq!(report.synthetic.upgraded, 0, "{report:?}");
    assert_eq!(report.synthetic.median_signup_to_first_call_secs, None);

    // `to_json` round-trips into the shape `mcphost funnel --json` prints.
    let json_body = report.to_json();
    assert_eq!(json_body["real"]["signed_up"], json!(2));
    assert_eq!(json_body["synthetic"]["signed_up"], json!(1));
}
