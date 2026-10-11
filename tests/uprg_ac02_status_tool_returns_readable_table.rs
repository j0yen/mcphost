//! PRD-mcphost-uptime-probe-recipe-green
//! AC2 -- Given AC1, When `host.tool_call(name="status")` runs, Then the result
//! has `columns`, three `rows`, and a `text` <= 2 KB that contains each URL (so
//! a fixture URL ending `probe-target-1` appears verbatim).

use crate::uprg;

use serde_json::json;

#[tokio::test]
async fn status_call_returns_columns_rows_and_a_text_table_with_every_url() {
    let fx = uprg::start().await;
    fx.create_status().await;

    let raw = fx.client.tools_call("host.tool_call", json!({"name": "status"})).await.expect("status call");
    let result = uprg::call_result(&raw);

    assert_eq!(result["columns"], json!(["url", "ok", "status", "latency_ms", "checked_at"]), "{result}");
    assert_eq!(result["rows"].as_array().map(Vec::len), Some(3), "{result}");
    let text = result["text"].as_str().unwrap_or_else(|| panic!("no text: {result}"));
    assert!(text.len() <= 2048, "text is {} bytes", text.len());
    for url in &fx.urls {
        assert!(text.contains(url.as_str()), "text must contain {url}:\n{text}");
    }
    assert!(fx.urls[0].ends_with("probe-target-1") && text.contains("probe-target-1"), "{text}");
}
