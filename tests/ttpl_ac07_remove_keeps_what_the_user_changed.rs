//! PRD-mcphost-tenant-templates
//! AC7 -- Given an applied template whose tool the user republished, When
//! `host.template_remove` runs, Then the trigger and table are removed, the tool
//! is kept and named under `kept`.

use crate::ttpl;
use serde_json::json;

const KIT: &str = r#"
title = "Kit"
one_line = "A tool, a table and a schedule."
plan_fit = "free"

[[tools]]
name = "kit_job"
kind = "jq"
[tools.spec]
program = "{ok: true}"

[[tables]]
name = "kit_data"
[tables.columns]
id = "integer"

[[triggers]]
tool = "kit_job"
kind = "schedule"
schedule = "0 * * * *"

[demo]
call = { tool = "host.tool_call", args = { name = "kit_job" } }
expect = { ok = true }
"#;

#[tokio::test]
async fn ttpl_ac07_remove_drops_the_trigger_and_table_and_keeps_a_republished_tool() {
    let fx = ttpl::start().await;
    let tenant = fx.tenant().await;
    let template = ttpl::fixture_template("kit", KIT);
    let applied = mcphost::templates::apply_template(&fx.server.state, &tenant, &template, &json!({"name": "kit"})).await.expect("apply");
    let trigger_id = applied["created"]["triggers"][0].as_str().expect("trigger id").to_string();
    let (tools, triggers, tables) = fx.inventory().await;
    assert_eq!((tools.len(), triggers.len(), tables.len()), (1, 1, 1), "the kit is applied");

    // The user republishes the tool: a new version the template did not make.
    fx.ok("host.tool_publish", json!({"name": "kit_job", "kind": "jq", "spec": {"program": "{ok: true, edited: true}"}})).await;

    let out = fx.ok("host.template_remove", json!({"name": "kit"})).await;
    assert_eq!(out["removed"]["triggers"], json!([trigger_id]), "{out}");
    assert_eq!(out["removed"]["tables"], json!(["kit_data"]), "{out}");
    assert_eq!(out["removed"]["tools"], json!([]), "{out}");
    assert_eq!(out["kept"]["tools"], json!(["kit_job"]), "{out}");

    let (tools, triggers, tables) = fx.inventory().await;
    assert_eq!((tools.len(), triggers, tables), (1, Vec::<String>::new(), Vec::<String>::new()), "only the user's tool is left");
    let tags = fx.server.state.db.template_objects(tenant.id, "kit".into()).await.expect("tags");
    assert!(tags.is_empty(), "a kept object is the user's now: {tags:?}");
}

#[tokio::test]
async fn ttpl_ac07_remove_takes_an_untouched_template_away_whole() {
    let fx = ttpl::start().await;
    fx.ok("host.template_apply", json!({"name": "scheduled-job"})).await;

    let out = fx.ok("host.template_remove", json!({"name": "scheduled-job"})).await;
    assert_eq!(out["removed"]["tools"], json!(["scheduled_job"]), "{out}");
    assert_eq!(out["kept"]["tools"], json!([]), "{out}");
    assert_eq!(fx.inventory().await, (vec![], vec![], vec![]), "nothing is left");
}
