//! PRD-mcphost-event-trigger-self-test
//! AC8 (P0) -- Given host.quickstart(kind="event"), When called, Then the
//! response carries `hint` containing `host.trigger.set(kind="event")`, and
//! host.quickstart()'s text plus llms.txt's first-run walkthrough contain
//! the sentence naming host.trigger.set for triggers.
//!
//! The AC was drafted when `kind="event"` made quickstart fail
//! `unknown_kind`, so its `hint` rode on that error.
//! PRD-mcphost-unknown-kind-routes-to-recipe landed on main since
//! (kindroute_ac01/ac02 assert `kind="event"` resolves successfully to the
//! http webhook-inbox recipe and must not regress), so the same `hint`
//! field with the same payload now rides on the success response. The
//! assertion below is the AC's own: the response to
//! `host.quickstart(kind="event")` contains the literal string
//! `host.trigger.set(kind="event")` in its `hint`. Without
//! `control::trigger_set_hint` there is no `hint` on that response at all
//! and this test fails.

use crate::common;
use common::{TempDataDir, TestServer, all_kinds_registry, extract_structured, signup};
use serde_json::json;

const LLMS_TXT: &str = include_str!("../www/llms.txt");

/// The exact call AC8 quotes.
const AC8_HINT_CALL: &str = r#"host.trigger.set(kind="event")"#;

#[tokio::test]
async fn quickstart_kind_event_hints_host_trigger_set() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(all_kinds_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "SelfTest AC8 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let structured = extract_structured(
        &client
            .tools_call("host.quickstart", json!({"kind": "event"}))
            .await
            .expect("quickstart kind=\"event\" resolves via the alias table"),
    );

    // The AC's own Then: the hint names host.trigger.set(kind="event").
    let hint = structured["hint"]
        .as_str()
        .unwrap_or_else(|| panic!("quickstart(kind=\"event\") must carry a hint: {structured}"));
    assert!(
        hint.contains(AC8_HINT_CALL),
        "hint must contain {AC8_HINT_CALL}: {hint}"
    );

    // ...and it is reached without the agent having to recover from a
    // rejection first (kindroute_ac01/ac02's landed behavior): the same
    // response resolves to the http recipe whose steps name the call.
    assert_eq!(structured["kind"], json!("http"));
    assert_eq!(structured["resolved_from"], json!("event"));
    let steps = structured["recipe"]["steps"]
        .as_array()
        .expect("recipe.steps array");
    let tools: Vec<&str> = steps.iter().filter_map(|s| s["tool"].as_str()).collect();
    assert!(
        tools.contains(&"host.trigger.set"),
        "recipe.steps must name host.trigger.set: {tools:?}"
    );

    // A trigger word the alias table knows but host.trigger.set does not
    // accept as a `kind` must not be hinted back verbatim -- "cron" is a
    // schedules-recipe alias, so the hint names kind="schedule".
    let cron = extract_structured(
        &client
            .tools_call("host.quickstart", json!({"kind": "cron"}))
            .await
            .expect("quickstart kind=\"cron\""),
    );
    let cron_hint = cron["hint"].as_str().expect("cron hint");
    assert!(
        cron_hint.contains(r#"host.trigger.set(kind="schedule")"#),
        "cron must hint a kind host.trigger.set accepts: {cron_hint}"
    );
}

#[tokio::test]
async fn quickstart_text_names_host_trigger_set_for_triggers() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(all_kinds_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "SelfTest AC8 Text Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let quickstart = extract_structured(
        &client
            .tools_call("host.quickstart", json!({}))
            .await
            .expect("host.quickstart() with no kind"),
    );
    let triggers = quickstart["triggers"].as_str().unwrap_or_else(|| {
        panic!("host.quickstart() must carry a triggers sentence: {quickstart}")
    });
    assert!(
        triggers.contains("host.trigger.set"),
        "the triggers sentence must name host.trigger.set: {triggers}"
    );
    assert!(
        triggers.contains("event"),
        "the triggers sentence must name the event trigger kind: {triggers}"
    );
}

#[test]
fn llms_txt_first_run_walkthrough_names_host_trigger_set() {
    let first_run = LLMS_TXT
        .find("## First run")
        .map(|start| &LLMS_TXT[start..])
        .unwrap_or_else(|| panic!("www/llms.txt must have a '## First run' section"));
    let walkthrough_end = first_run.find("## Connect over OAuth").unwrap_or(first_run.len());
    let section = &first_run[..walkthrough_end];
    assert!(
        section.contains("host.trigger.set"),
        "the First run walkthrough must name host.trigger.set for triggers"
    );
}
