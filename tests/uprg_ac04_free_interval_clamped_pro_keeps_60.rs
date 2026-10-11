//! PRD-mcphost-uptime-probe-recipe-green
//! AC4 -- Given a free tenant and `every_s: 60`, When created, Then the
//! schedule interval is 300 and the response carries `data.clamped_to: 300`;
//! Given a pro tenant, Then 60 is kept.

use crate::common;
use crate::uprg;

use common::{McpClient, extract_structured, signup_and_make_pro};
use serde_json::json;

#[tokio::test]
async fn free_tenant_every_60_is_clamped_to_300() {
    let fx = uprg::start().await;
    let out = fx.create(json!({"name": "status", "urls": fx.urls, "every_s": 60})).await.expect("create");
    assert_eq!(out["every_s"], json!(300), "{out}");
    assert_eq!(out["data"]["clamped_to"], json!(300), "{out}");

    let triggers = extract_structured(&fx.client.tools_call("host.trigger.list", json!({})).await.expect("list"));
    assert_eq!(triggers["triggers"][0]["schedule"], json!("*/5 * * * *"), "{triggers}");
}

#[tokio::test]
async fn pro_tenant_keeps_60_with_no_clamp_note() {
    let fx = uprg::start().await;
    let (_ns, key, _id) = signup_and_make_pro(&fx._server, "uprg pro", "cus_uprg_ac04").await;
    let pro = McpClient::with_bearer(&fx._server.base_url, &key);
    let out = extract_structured(
        &pro.tools_call("host.uptime.create", json!({"name": "status", "urls": fx.urls, "every_s": 60}))
            .await
            .expect("pro create"),
    );
    assert_eq!(out["every_s"], json!(60), "{out}");
    assert!(out.get("data").is_none() && out.get("clamped_to").is_none(), "{out}");
    let triggers = extract_structured(&pro.tools_call("host.trigger.list", json!({})).await.expect("list"));
    assert_eq!(triggers["triggers"][0]["schedule"], json!("* * * * *"), "{triggers}");
}
