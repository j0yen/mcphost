//! PRD-mcphost-synthetic-flag
//! AC6 — Given 10 tenants of which 6 match `name_like: '%Chen%'`, When
//! `admin.tenants_set_synthetic` runs with `dry_run: true`, Then the 6 are
//! listed and no row changes; and When rerun with `dry_run: false`, Then
//! exactly those 6 store the label.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn dry_run_previews_then_apply_tags_exactly_the_matches() {
    // Ten signups from the same source IP against the default 5/hour rate
    // limit -- this AC is about the bulk retro-tag, not signup throughput.
    let server = TestServer::start_with_signup_rate_limit(20).await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    let mut chen_namespaces = Vec::new();
    for i in 0..6 {
        let (ns, _) = signup(&server.base_url, &format!("Alex Chen {i}")).await;
        chen_namespaces.push(ns);
    }
    let mut other_namespaces = Vec::new();
    for i in 0..4 {
        let (ns, _) = signup(&server.base_url, &format!("Real User {i}")).await;
        other_namespaces.push(ns);
    }

    let dry_run_result = admin
        .tools_call(
            "admin.tenants_set_synthetic",
            json!({
                "name_like": "%Chen%",
                "label": "synthorg:backfill-20260906",
                "dry_run": true,
            }),
        )
        .await
        .expect("dry run");
    let dry_run = extract_structured(&dry_run_result);
    assert_eq!(dry_run["dry_run"], json!(true));
    assert_eq!(dry_run["count"], json!(6));
    let matched = dry_run["matched"].as_array().expect("matched array");
    assert_eq!(matched.len(), 6);
    let matched_names: Vec<&str> = matched.iter().map(|m| m["tenant"].as_str().unwrap()).collect();
    for ns in &chen_namespaces {
        assert!(matched_names.contains(&ns.as_str()), "{ns} must be matched");
    }

    // Dry run must not have written anything. PRD-mcphost-tenant-attribution
    // requirement 1 / AC1: every signup here came from this suite's
    // loopback test server with no explicit stamp, so the untouched value
    // is `harness:unstamped`, not `None` -- the dry run's job is to leave
    // that default value exactly as signup left it.
    for ns in chen_namespaces.iter().chain(other_namespaces.iter()) {
        let tenant = server
            .state
            .db
            .find_tenant_by_namespace(ns.clone())
            .await
            .expect("query")
            .expect("tenant exists");
        assert_eq!(
            tenant.synthetic.as_deref(),
            Some("harness:unstamped"),
            "{ns} must be untouched by dry run"
        );
    }

    let apply_result = admin
        .tools_call(
            "admin.tenants_set_synthetic",
            json!({
                "name_like": "%Chen%",
                "label": "synthorg:backfill-20260906",
                "dry_run": false,
            }),
        )
        .await
        .expect("apply");
    let applied = extract_structured(&apply_result);
    assert_eq!(applied["dry_run"], json!(false));
    assert_eq!(applied["count"], json!(6));

    for ns in &chen_namespaces {
        let tenant = server
            .state
            .db
            .find_tenant_by_namespace(ns.clone())
            .await
            .expect("query")
            .expect("tenant exists");
        assert_eq!(
            tenant.synthetic.as_deref(),
            Some("synthorg:backfill-20260906"),
            "{ns} must now be labeled"
        );
    }
    for ns in &other_namespaces {
        let tenant = server
            .state
            .db
            .find_tenant_by_namespace(ns.clone())
            .await
            .expect("query")
            .expect("tenant exists");
        assert_eq!(
            tenant.synthetic.as_deref(),
            Some("harness:unstamped"),
            "{ns} must remain at its signup-time default, untouched by the name_like apply"
        );
    }
}
