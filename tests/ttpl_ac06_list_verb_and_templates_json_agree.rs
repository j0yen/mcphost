//! PRD-mcphost-tenant-templates
//! AC6 -- Given anonymous and keyed `host.template_list` and
//! `GET /templates.json`, When called, Then all three return the same five
//! names with `one_line`, `creates` counts and `plan_fit`, and keyed
//! `host.quickstart` without `kind` carries `templates`.

use crate::common::{McpClient, extract_structured};
use crate::ttpl;
use serde_json::{Value, json};

const FIVE: [&str; 5] = ["database-in-a-minute", "docs-qa", "scheduled-job", "uptime-probes", "webhook-inbox"];

fn names(list: &Value) -> Vec<String> {
    let mut names: Vec<String> = list["templates"].as_array().expect("templates").iter().map(|t| t["name"].as_str().expect("name").to_string()).collect();
    names.sort();
    names
}

#[tokio::test]
async fn ttpl_ac06_anonymous_keyed_and_http_listings_are_the_same_five_templates() {
    let fx = ttpl::start().await;

    let anonymous = extract_structured(
        &McpClient::new(&fx.server.base_url).tools_call("host.template_list", json!({})).await.expect("anonymous template_list"),
    );
    let keyed = fx.ok("host.template_list", json!({})).await;
    let http: Value = reqwest::get(format!("{}/templates.json", fx.server.base_url))
        .await
        .expect("GET /templates.json")
        .json()
        .await
        .expect("json");

    assert_eq!(names(&keyed), FIVE, "{keyed}");
    assert_eq!(anonymous, keyed, "anonymous and keyed listings are the same");
    assert_eq!(http, keyed, "GET /templates.json serves the same list");
    for t in keyed["templates"].as_array().expect("templates") {
        assert!(t["one_line"].as_str().is_some_and(|s| !s.is_empty()), "{t}");
        assert!(t["plan_fit"].as_str().is_some_and(|s| !s.is_empty()), "{t}");
        for kind in ["tools", "tables", "triggers"] {
            assert!(t["creates"][kind].is_u64(), "creates.{kind} is a count: {t}");
        }
    }

    let quickstart = fx.ok("host.quickstart", json!({})).await;
    assert_eq!(quickstart["authenticated"], json!(true), "{quickstart}");
    let carried: Vec<&str> = quickstart["templates"].as_array().expect("quickstart.templates").iter().filter_map(|t| t["name"].as_str()).collect();
    let mut carried: Vec<String> = carried.into_iter().map(str::to_string).collect();
    carried.sort();
    assert_eq!(carried, FIVE, "{quickstart}");
    assert!(!quickstart["recipes"].is_null(), "templates sits beside the existing recipes key");
}

#[tokio::test]
async fn ttpl_ac06_the_data_template_applies_and_its_demo_call_returns_its_expectation() {
    let fx = ttpl::start().await;
    let applied = fx.ok("host.template_apply", json!({"name": "database-in-a-minute", "params": {"name": "ledger"}})).await;
    assert_eq!(applied["created"]["tables"], json!(["ledger"]), "{applied}");
    let demo = &applied["demo"];
    let result = fx.ok(demo["call"]["tool"].as_str().expect("demo tool"), demo["call"]["args"].clone()).await;
    assert!(ttpl::contains_value(&result, &demo["expect"]), "demo.expect {} not in {result}", demo["expect"]);
}

#[tokio::test]
async fn ttpl_ac06_the_uptime_template_wraps_host_uptime_create_and_reapplies_in_place() {
    let fx = ttpl::start().await;
    let (_targets, urls) = ttpl::probe_targets().await;
    let params = json!({"name": "site_status", "urls": urls});
    let apply = || fx.client.tools_call("host.template_apply", json!({"name": "uptime-probes", "params": params}));

    let applied = extract_structured(&apply().await.expect("apply"));
    assert_eq!(applied["created"]["tools"], json!(["site_status", "site_status_probe"]), "{applied}");
    assert_eq!(applied["created"]["tables"], json!(["site_status_targets"]), "{applied}");
    assert_eq!(applied["created"]["triggers"].as_array().map(Vec::len), Some(1), "{applied}");
    let before = fx.inventory().await;

    let demo = &applied["demo"];
    let result = extract_structured(&fx.client.tools_call(demo["call"]["tool"].as_str().expect("tool"), demo["call"]["args"].clone()).await.expect("demo call"));
    assert!(ttpl::contains_value(&result, &demo["expect"]), "demo.expect {} not in {result}", demo["expect"]);

    let again = extract_structured(&apply().await.expect("re-apply"));
    assert_eq!(again["created"]["tools"], json!([]), "{again}");
    assert_eq!(again["updated"]["tools"], json!(["site_status", "site_status_probe"]), "{again}");
    let after = fx.inventory().await;
    assert_eq!((after.0, after.1.len(), after.2), (before.0, before.1.len(), before.2), "no duplicate tool, table or schedule");
}
