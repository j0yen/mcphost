//! PRD-mcphost-tenant-templates
//! AC4 -- Given a tool create that fails midway (planted invalid spec in a
//! test template), When apply runs, Then the response is the create's error and
//! no object tagged with that template remains.

use crate::ttpl;
use serde_json::json;

const FIXTURE: &str = r#"
title = "Half broken"
one_line = "Two tools; the second one's program does not parse."
plan_fit = "free"

[[tools]]
name = "good_job"
kind = "jq"
[tools.spec]
program = "."

[[tools]]
name = "bad_job"
kind = "jq"
[tools.spec]
program = "}{ not a jq program"

[[triggers]]
tool = "good_job"
kind = "schedule"
schedule = "0 * * * *"

[demo]
call = { tool = "host.tool_call", args = { name = "good_job" } }
expect = { ok = true }
"#;

#[tokio::test]
async fn ttpl_ac04_a_failed_create_returns_its_error_and_leaves_nothing_tagged() {
    let fx = ttpl::start().await;
    let before = fx.inventory().await;
    let tenant = fx.tenant().await;
    let template = ttpl::fixture_template("half-broken", FIXTURE);

    let err = mcphost::templates::apply_template(&fx.server.state, &tenant, &template, &json!({"name": "half-broken"}))
        .await
        .expect_err("the second tool's spec is invalid");

    // The create's own error: the same one a direct publish of that spec answers.
    let direct = fx
        .call("host.tool_publish", json!({"name": "bad_job", "kind": "jq", "spec": {"program": "}{ not a jq program"}}))
        .await
        .expect_err("a direct publish of the same spec fails");
    assert_eq!(err.code(), direct.error_code.as_deref().expect("direct error code"), "{err}");

    let tagged = fx.server.state.db.template_objects(tenant.id, "half-broken".into()).await.expect("tags");
    assert!(tagged.is_empty(), "no object tagged with the template remains: {tagged:?}");
    assert_eq!(fx.inventory().await, before, "the first tool was removed again");
}
