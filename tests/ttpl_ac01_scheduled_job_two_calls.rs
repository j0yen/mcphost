//! PRD-mcphost-tenant-templates
//! AC1 -- Given a fresh free tenant and a test call log, When
//! `host.template_apply(name="scheduled-job")` runs and then the returned
//! `demo.call` runs, Then a result equal to `demo.expect` is in the second
//! call's result and the log holds exactly 2 calls; a third required call
//! fails the test (premise falsified).

use crate::ttpl;
use serde_json::json;

#[tokio::test]
async fn ttpl_ac01_scheduled_job_first_result_in_two_calls() {
    let fx = ttpl::start().await;

    let applied = fx.ok("host.template_apply", json!({"name": "scheduled-job"})).await;
    let demo = &applied["demo"];
    let tool = demo["call"]["tool"].as_str().unwrap_or_else(|| panic!("no demo.call.tool: {applied}"));
    let second = fx.ok(tool, demo["call"]["args"].clone()).await;

    assert!(
        ttpl::contains_value(&second, &demo["expect"]),
        "demo.expect {} is not in the demo call's result {second}",
        demo["expect"]
    );
    assert_eq!(fx.calls(), vec!["host.template_apply".to_string(), tool.to_string()], "the premise is two calls, not three");
    assert_eq!(applied["created"]["tools"], json!(["scheduled_job"]), "{applied}");
    assert_eq!(applied["created"]["triggers"].as_array().map(Vec::len), Some(1), "{applied}");
}
