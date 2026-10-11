//! PRD-mcphost-webhook-inbox-one-call
//! AC7 -- Given a tenant at `event_triggers_max`, When
//! `host.webhook_inbox.create` is called, Then the error is `plan_limit` with
//! `data.limit == 3` and no tool, trigger or event row was created; Given a
//! second create with the same name, Then the same `url` returns and
//! `secret` is absent.

use crate::common;
use crate::whin;

use common::extract_structured;
use mcphost::sandbox;
use serde_json::{Value, json};

async fn names(fx: &whin::Fixture, tool: &str, key: &str) -> Vec<String> {
    let out = extract_structured(&fx.client.tools_call(tool, json!({})).await.expect(tool));
    out[key]
        .as_array()
        .unwrap_or_else(|| panic!("{tool}: {out}"))
        .iter()
        .filter_map(|t| t["name"].as_str().or_else(|| t["tool"].as_str()).map(str::to_string))
        .collect()
}

#[tokio::test]
async fn at_the_ceiling_create_is_plan_limit_with_nothing_created_and_a_repeat_returns_the_same_url() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let fx = whin::start().await;
    let ceiling = mcphost::plans::PlanCatalog::default_catalog()
        .get("free")
        .expect("free plan")
        .event_triggers_max;
    assert_eq!(ceiling, 3, "the PRD's number");

    let mut first: Option<Value> = None;
    for i in 0..ceiling {
        let out = fx.create(json!({"name": format!("inbox_{i}")})).await.unwrap_or_else(|e| panic!("{i}: {}", e.message));
        if first.is_none() {
            first = Some(out);
        }
    }
    let first = first.expect("first inbox");
    let tools_before = names(&fx, "host.tool_list", "tools").await.len();
    let triggers_before = names(&fx, "host.trigger.list", "triggers").await.len();
    assert_eq!(triggers_before, ceiling as usize);

    let refused = fx
        .create(json!({"name": "one_too_many"}))
        .await
        .expect_err("a create at the ceiling must be refused");
    assert_eq!(refused.error_code.as_deref(), Some("plan_limit"), "{refused:?}");
    assert_eq!(refused.data["limit"], json!(3), "{refused:?}");

    assert_eq!(names(&fx, "host.tool_list", "tools").await.len(), tools_before, "no tool created");
    assert_eq!(names(&fx, "host.trigger.list", "triggers").await.len(), triggers_before, "no trigger created");
    let table = fx
        .client
        .tools_call("host.state.query", json!({"table": "inbox_one_too_many"}))
        .await
        .expect_err("no event table created");
    assert_eq!(table.error_code.as_deref(), Some("state_table_not_found"), "{table:?}");

    // A repeat of an existing name is not a create: same url, no secret.
    let again = fx.create(json!({"name": "inbox_0"})).await.expect("repeat create");
    assert_eq!(again["url"], first["url"], "{again}");
    assert!(again.get("secret").is_none(), "secret must be absent on a repeat: {again}");
    assert_eq!(names(&fx, "host.trigger.list", "triggers").await.len(), triggers_before);

    // `rotate: true` is the only way to see a secret again; the url is stable.
    let rotated = fx.create(json!({"name": "inbox_0", "rotate": true})).await.expect("rotate");
    assert_eq!(rotated["url"], first["url"], "{rotated}");
    assert_ne!(rotated["secret"], first["secret"], "{rotated}");
    assert!(rotated["secret"].as_str().is_some_and(|s| !s.is_empty()), "{rotated}");
}
