//! PRD-mcphost-host-doctor-tool-b
//! AC1 -- Given tenant A's key with `tenant: <B>`, When `host.doctor` runs,
//! Then `forbidden`; Given the admin bearer with `tenant: <B>`, Then B's groups.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn tenant_key_naming_another_tenant_is_forbidden_but_admin_reads_it() {
    let server = TestServer::start().await;
    let (_, key_a) = signup(&server.base_url, "hostdocb ac1 a").await;
    let (ns_b, key_b) = signup(&server.base_url, "hostdocb ac1 b").await;
    let tenant_b = server.state.db.find_tenant_by_namespace(ns_b.clone()).await.expect("lookup").expect("tenant b");
    for _ in 0..3 {
        server
            .state
            .db
            .record_call_traced(
                tenant_b.id,
                "lookup".to_string(),
                9,
                false,
                Some("tool_exception".to_string()),
                None,
                None,
                "error",
                tenant_b.origin.clone(),
                tenant_b.origin_detail.clone(),
                None,
                None,
                None,
                None,
                None,
                "key".to_string(),
                None,
                None,
                None,
                Some("tool_exception: TypeError: string indices must be integers at tool.py:16\nname = rows[\"name\"]".to_string()),
            )
            .await
            .expect("record failed call");
    }

    let a = McpClient::with_bearer(&server.base_url, &key_a);
    let err = a.tools_call("host.doctor", json!({"tenant": ns_b})).await.expect_err("A reading B");
    assert_eq!(err.error_code.as_deref(), Some("forbidden"), "{} {}", err.code, err.message);

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let out = extract_structured(&admin.tools_call("host.doctor", json!({"tenant": ns_b})).await.expect("admin"));
    assert_eq!(out["window"]["failed"], json!(3), "{out}");
    let groups = out["groups"].as_array().expect("groups");
    assert_eq!(groups.len(), 1, "{out}");
    assert_eq!(groups[0]["code"], json!("tool_exception"), "{out}");
    assert_eq!(groups[0]["count"], json!(3), "{out}");

    // B's own key reads the same groups.
    let b = McpClient::with_bearer(&server.base_url, &key_b);
    let own = extract_structured(&b.tools_call("host.doctor", json!({})).await.expect("own"));
    assert_eq!(own["groups"], out["groups"], "{own}");
}
