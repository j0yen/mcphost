//! PRD-mcphost-chain-fanout-and-join AC6 (P0) -- Given `Plan`, When a test
//! renders `ceilings_json` for every plan and reads `host.quickstart
//! kind=chain`, Then `limits.plan.chain_fanout_max` is present for each and
//! the quickstart example contains one `for_each` step that publishes and
//! passes `host.tool_test`.

use crate::common;
use common::{McpClient, TestServer, chain_kind_registry, extract_structured, publish, signup};
use mcphost::plans::PlanCatalog;
use serde_json::json;

#[tokio::test]
async fn every_plan_renders_the_ceiling_and_the_example_fans_out() {
    let catalog = PlanCatalog::default_catalog();
    assert!(!catalog.plans.is_empty());
    for plan in &catalog.plans {
        let rendered = plan.ceilings_json();
        assert_eq!(rendered["chain_fanout_max"], json!(plan.chain_fanout_max), "{}: {rendered}", plan.name);
    }
    assert_eq!(catalog.get("free").expect("free").chain_fanout_max, 20);
    assert_eq!(catalog.get("pro").expect("pro").chain_fanout_max, 50);

    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Cfan AC6 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let quickstart = extract_structured(
        &client.tools_call("host.quickstart", json!({"kind": "chain"})).await.expect("host.quickstart kind=chain"),
    );
    assert_eq!(quickstart["limits"]["plan"]["chain_fanout_max"], json!(20), "{quickstart}");

    let publish_step = &quickstart["steps"][0];
    let spec = publish_step["arguments"]["spec"].clone();
    let name = publish_step["arguments"]["name"].as_str().expect("example chain name").to_string();
    let steps = spec["steps"].as_array().expect("example steps");
    let fanouts: Vec<_> = steps.iter().filter(|s| s.get("for_each").is_some()).collect();
    assert_eq!(fanouts.len(), 1, "the example carries exactly one for_each step: {spec}");

    // Publish every tool the example names (as echo stand-ins, so no sandbox
    // is needed), then the example itself, exactly as printed.
    for step in steps {
        let tool = step["tool"].as_str().expect("step tool");
        if !tool.starts_with("host.") {
            publish(&client, tool, "echo", json!({"schema": {"type": "object"}})).await;
        }
    }
    publish(&client, &name, "chain", spec).await;

    let args = quickstart["steps"]
        .as_array()
        .expect("quickstart steps")
        .iter()
        .find(|s| s["call"] == json!("host.tool_call"))
        .map(|s| s["arguments"]["args"].clone())
        .unwrap_or_else(|| json!({"text": "hello", "notes": ["first", "second"]}));
    let tested = extract_structured(
        &client
            .tools_call(
                "host.tool_test",
                // Step 1 is an echo stand-in for the python starter here, so
                // its recorded result is the starter's real return shape.
                json!({"name": name, "args": args,
                       "fixtures": {"1": {"appended": 1, "table": "quickstart_notes", "total": 1}}}),
            )
            .await
            .expect("host.tool_test on the quickstart chain example"),
    );
    assert_eq!(tested["verdict"], json!("pass"), "{tested}");
    let reported = tested["steps"]
        .as_array()
        .expect("dry-run steps")
        .iter()
        .find(|s| s.get("fanout").is_some())
        .unwrap_or_else(|| panic!("the for_each step is reported with a fanout block: {tested}"));
    assert_eq!(reported["fanout"]["limit"], json!(20), "{tested}");
    assert_eq!(reported["fanout"]["path"], json!("$.input.notes"), "{tested}");
    assert_eq!(reported["resolved_args"]["note"], json!("first"), "the first item's args: {tested}");
}
