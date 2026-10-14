//! PRD-mcphost-time-to-first-success-public AC4 (P0) — Given any
//! `onboarding_7d` block, When its JSON is walked, Then every leaf is a
//! number or null (`computed_at` is a unix number), and no string equals any
//! tenant namespace, client name or IP in the fixture.

use crate::common;
use common::{TestServer, ttfs_tenant};
use serde_json::{Value, json};

fn walk(v: &Value, path: &str, strings: &mut Vec<String>) {
    match v {
        Value::Number(_) | Value::Null => {}
        Value::Object(map) => {
            for (k, child) in map {
                strings.push(k.clone());
                walk(child, &format!("{path}.{k}"), strings);
            }
        }
        other => panic!("leaf at {path} is not a number or null: {other}"),
    }
}

#[tokio::test]
async fn every_leaf_is_numeric_and_no_fixture_identifier_appears() {
    let server = TestServer::start().await;
    let db = &server.state.db;
    let now = mcphost::state::now_unix();
    let mut namespaces = Vec::new();
    for i in 0..6 {
        let ns = format!("ttfs4-secret-ns-{i}");
        ttfs_tenant(db, &ns, "external", Some("Cursor-ttfs4-secret"), now - 600, [Some(1), Some(20), Some(30), None]).await;
        namespaces.push(ns);
    }
    // A signup that went through the handler, so an IP is on record.
    let signed = mcphost::control::signup(
        &server.state,
        &json!({"name": "ttfs4-named"}),
        "8.8.4.4",
        mcphost::control::SignupAttribution::default(),
    )
    .await
    .expect("signup");
    namespaces.push(signed["tenant"].as_str().expect("namespace").to_string());

    let block = mcphost::funnel::onboarding_7d(db, now, mcphost::funnel::ONBOARDING_MIN_N).await.expect("compute");
    assert!(block["p50_s_to_first_own_call"].is_number(), "fixture must exercise real percentiles: {block}");
    let mut strings = Vec::new();
    walk(&block, "$", &mut strings);
    assert_eq!(block["computed_at"], json!(now));

    let mut forbidden = namespaces;
    forbidden.extend(["Cursor-ttfs4-secret".into(), "8.8.4.4".into(), "ttfs4-named".into()]);
    for s in &strings {
        assert!(!forbidden.contains(s), "identifier {s} leaked into {block}");
    }
    let text = block.to_string();
    for f in &forbidden {
        assert!(!text.contains(f.as_str()), "identifier {f} appears in {text}");
    }
}
