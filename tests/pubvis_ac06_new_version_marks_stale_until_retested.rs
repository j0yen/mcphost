//! PRD-mcphost-publish-visibility-and-tags AC6 (P0) -- Given a public tool at
//! version 1 with `tested_version 1`, When a new version is published, Then
//! it stays public with `stale_test: true`, and a passing `host.tool_test`
//! sets `tested_version: 2` and clears the flag.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn pubvis_ac06() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Pubvis AC6 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let spec = json!({"schema": {"type": "object"}});

    let v1 = extract_structured(
        &client
            .tools_call(
                "host.tool_publish",
                json!({"name": "versioned", "kind": "echo", "spec": spec, "visibility": "public"}),
            )
            .await
            .expect("v1"),
    );
    assert_eq!((v1["version"].clone(), v1["tested_version"].clone()), (json!(1), json!(1)), "{v1}");
    assert_eq!(v1["stale_test"], json!(false), "{v1}");

    let v2 = extract_structured(
        &client
            .tools_call(
                "host.tool_publish",
                json!({"name": "versioned", "kind": "echo", "spec": {"schema": {"type": "object"}, "description": "v2"}}),
            )
            .await
            .expect("v2"),
    );
    assert_eq!(v2["version"], json!(2), "{v2}");
    assert_eq!(v2["visibility"], json!("public"), "a new version stays public: {v2}");
    assert_eq!(v2["stale_test"], json!(true), "{v2}");
    assert_eq!(v2["tested_version"], json!(1), "{v2}");

    let full = format!("{ns}.versioned");
    let got = extract_structured(&client.tools_call("host.catalog.get", json!({"full_name": full})).await.expect("get"));
    assert_eq!(got["stale_test"], json!(true), "readers see the stale flag: {got}");

    let tested = extract_structured(
        &client.tools_call("host.tool_test", json!({"name": "versioned", "args": {}})).await.expect("tool_test"),
    );
    assert_eq!(tested["verdict"], json!("pass"), "{tested}");
    assert_eq!(tested["tested_version"], json!(2), "{tested}");
    assert_eq!(tested["stale_test"], json!(false), "{tested}");

    let got = extract_structured(&client.tools_call("host.catalog.get", json!({"full_name": full})).await.expect("get"));
    assert_eq!((got["tested_version"].clone(), got["stale_test"].clone()), (json!(2), json!(false)), "{got}");
}
