//! contract: reads registry handler::host_tool_descriptors
//! PRD-mcphost-notify-user-channel
//! AC2 -- Given `ChannelKind` and `Level`, When the tool schemas, llms.txt,
//! delivery dispatch and `/help/notify_kind_unknown` are rendered, Then each
//! lists exactly the enum names in order and a copy with an extra variant
//! fails the drift test.

use crate::common;
use common::TestServer;
use mcphost::notify::{ChannelKind, Level, check_exact, dispatch_kinds};
use serde_json::Value;

fn schema_enum(tools: &[rmcp::model::Tool], tool: &str, field: &str) -> Vec<String> {
    let t = tools.iter().find(|t| t.name.as_ref() == tool).unwrap_or_else(|| panic!("{tool} is not listed"));
    t.input_schema["properties"][field]["enum"]
        .as_array()
        .unwrap_or_else(|| panic!("{tool}.{field} has no enum"))
        .iter()
        .map(|v| v.as_str().expect("enum string").to_string())
        .collect()
}

/// The backticked names between `after` and the next `)` in `text`.
fn names_after(text: &str, after: &str) -> Vec<String> {
    let rest = text.split(after).nth(1).unwrap_or_else(|| panic!("no '{after}' in: {text}"));
    let list = rest.split(')').next().unwrap_or("");
    list.split(',').map(|s| s.trim().trim_matches('`').to_string()).filter(|s| !s.is_empty()).collect()
}

/// The comma-separated names after `marker`, up to the sentence's full stop.
fn sentence_list_after(text: &str, marker: &str) -> Vec<String> {
    let rest = text.split(marker).nth(1).unwrap_or_else(|| panic!("no '{marker}' in: {text}"));
    let list = rest.split('.').next().unwrap_or("");
    list.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
}

#[tokio::test]
async fn ntfy_ac02() {
    let kinds = ChannelKind::names();
    let levels = Level::names();
    assert_eq!(kinds, ["email", "webhook", "slack"]);
    assert_eq!(levels, ["info", "warn", "action"]);

    // 1. Tool schemas.
    let tools = mcphost::handler::host_tool_descriptors(&common::chain_kind_registry());
    check_exact("channel_add schema", &schema_enum(&tools, "host.notify.channel_add", "kind"), &kinds).expect("schema kinds");
    check_exact("send schema", &schema_enum(&tools, "host.notify.send", "level"), &levels).expect("schema levels");

    // 2. llms.txt: the committed file and the renderer agree with the enums.
    let committed = include_str!("../www/llms.txt");
    assert!(committed.contains(mcphost::notify::SECTION_START), "www/llms.txt lacks the notify block");
    for text in [committed.to_string(), mcphost::notify::render_section()] {
        check_exact("llms.txt kinds", &names_after(&text, "kind is one of "), &kinds).expect("llms kinds");
        check_exact("llms.txt levels", &names_after(&text, "level is one of "), &levels).expect("llms levels");
    }

    // 3. Delivery dispatch.
    let dispatch: Vec<String> = dispatch_kinds().into_iter().map(String::from).collect();
    check_exact("delivery dispatch", &dispatch, &kinds).expect("dispatch kinds");

    // 4. /help/notify_kind_unknown over HTTP.
    let server = TestServer::start().await;
    let resp = reqwest::get(format!("{}/help/notify_kind_unknown", server.base_url)).await.expect("GET help");
    assert_eq!(resp.status(), 200);
    let html = resp.text().await.expect("help body");
    let help_kinds = sentence_list_after(&html, "The kinds are: ");
    check_exact("help page kinds", &help_kinds, &kinds).expect("help kinds");
    let help_levels = sentence_list_after(&html, "host.notify.send accepts are ");
    check_exact("help page levels", &help_levels, &levels).expect("help levels");

    // The unknown-kind refusal itself carries the same list.
    let err = mcphost::notify::kind_unknown("sms").into_error_data();
    let data: &Value = err.data.as_ref().expect("error data");
    let carried: Vec<String> = data["kinds"].as_array().expect("kinds").iter().map(|v| v.as_str().unwrap().to_string()).collect();
    check_exact("notify_kind_unknown data.kinds", &carried, &kinds).expect("error kinds");

    // Drift: a copy with an extra variant fails every comparison above.
    let mut with_extra: Vec<String> = kinds.iter().map(|s| s.to_string()).collect();
    with_extra.push("discord".to_string());
    assert!(check_exact("copy", &with_extra, &kinds).is_err(), "an extra variant must fail the drift test");
    let mut reordered: Vec<String> = kinds.iter().map(|s| s.to_string()).collect();
    reordered.reverse();
    assert!(check_exact("copy", &reordered, &kinds).is_err(), "a reordered copy must fail the drift test");
    let missing: Vec<String> = levels.iter().skip(1).map(|s| s.to_string()).collect();
    assert!(check_exact("copy", &missing, &levels).is_err(), "a missing variant must fail the drift test");
}
