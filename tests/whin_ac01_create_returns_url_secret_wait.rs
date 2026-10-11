//! PRD-mcphost-webhook-inbox-one-call
//! AC1 -- Given a free tenant, When `host.webhook_inbox.create(name="inbox")`
//! is called once, Then the response carries `url` matching
//! `^https://[^/]+/hook/[A-Za-z0-9]{10,}$`, `secret`, `tool: "inbox"`,
//! `trigger_id`, and `wait.tool == "host.msg.wait"`, and `host.trigger.list`
//! shows exactly one webhook trigger bound to `inbox`.

use crate::common;
use crate::whin;

use common::extract_structured;
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn one_create_call_returns_url_secret_tool_trigger_and_wait() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let fx = whin::start().await;
    let out = fx.create_inbox().await;

    let url = out["url"].as_str().unwrap_or_else(|| panic!("no url: {out}"));
    let rest = url.strip_prefix("https://").unwrap_or_else(|| panic!("url is not https: {url}"));
    let (host, id) = rest.split_once("/hook/").unwrap_or_else(|| panic!("url has no /hook/: {url}"));
    assert!(!host.is_empty() && !host.contains('/'), "{url}");
    assert!(id.len() >= 10 && id.chars().all(|c| c.is_ascii_alphanumeric()), "{url}");

    assert!(out["secret"].as_str().is_some_and(|s| !s.is_empty()), "{out}");
    assert_eq!(out["tool"], json!("inbox"), "{out}");
    let trigger_id = out["trigger_id"].as_str().unwrap_or_else(|| panic!("no trigger_id: {out}"));
    assert_eq!(out["wait"]["tool"], json!("host.msg.wait"), "{out}");
    assert!(out["wait"]["args"].is_object(), "{out}");

    let listed = extract_structured(&fx.client.tools_call("host.trigger.list", json!({})).await.expect("trigger.list"));
    let webhooks: Vec<_> = listed["triggers"]
        .as_array()
        .unwrap_or_else(|| panic!("triggers shape: {listed}"))
        .iter()
        .filter(|t| t["kind"] == json!("webhook"))
        .collect();
    assert_eq!(webhooks.len(), 1, "{listed}");
    assert_eq!(webhooks[0]["tool"], json!("inbox"), "{listed}");
    assert_eq!(webhooks[0]["id"], json!(trigger_id), "{listed}");
}
