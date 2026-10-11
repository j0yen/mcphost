//! PRD-mcphost-uptime-probe-recipe-green
//! AC3 -- Given 21 URLs, an empty list, or a URL whose host fails
//! `is_disallowed_literal_host`, When `host.uptime.create` is called, Then the
//! error is `args_invalid` (naming `max 20` or `empty`) or `host_not_allowed`
//! naming the URL, and nothing was created.

use crate::uprg;

use serde_json::json;

async fn assert_nothing_created(fx: &uprg::Fixture) {
    let (tools, triggers, tables) = uprg::inventory(&fx.client).await;
    assert!(tools.iter().all(|n| !n.contains("status")), "tools: {tools:?}");
    assert_eq!(triggers, 0);
    assert!(tables.iter().all(|n| !n.contains("status")), "tables: {tables:?}");
}

#[tokio::test]
async fn twenty_one_urls_is_args_invalid_naming_max_20() {
    let fx = uprg::start().await;
    let urls: Vec<String> = (0..21).map(|i| format!("{}?n={i}", fx.urls[0])).collect();
    let err = fx.create(json!({"name": "status", "urls": urls})).await.expect_err("21 urls");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{err:?}");
    assert!(err.message.contains("max 20"), "{err:?}");
    assert_nothing_created(&fx).await;
}

#[tokio::test]
async fn empty_list_is_args_invalid_naming_empty() {
    let fx = uprg::start().await;
    let err = fx.create(json!({"name": "status", "urls": []})).await.expect_err("no urls");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{err:?}");
    assert!(err.message.contains("empty"), "{err:?}");
    assert_nothing_created(&fx).await;
}

#[tokio::test]
async fn disallowed_host_is_host_not_allowed_naming_the_url() {
    let fx = uprg::start().await;
    let bad = "http://metadata.internal/probe-target-9";
    let urls = vec![fx.urls[0].clone(), bad.to_string()];
    let err = fx.create(json!({"name": "status", "urls": urls})).await.expect_err("internal host");
    assert_eq!(err.error_code.as_deref(), Some("host_not_allowed"), "{err:?}");
    assert!(err.message.contains(bad), "{err:?}");
    assert_nothing_created(&fx).await;
}
