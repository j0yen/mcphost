//! PRD-mcphost-tool-reach-manifest-v2
//! AC7 (P0) -- Given a sub-key whose allowlist holds only tool `a`, When it
//! calls `host.tool.reach` for `a` and for `b` (named `tool_a` and `tool_b`:
//! a tool name is at least two characters), Then `a` returns the caller
//! view with no secret names and `b` is `permission_denied`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
use serde_json::{Value, json};

async fn ok(client: &McpClient, verb: &str, args: Value) -> Value {
    extract_structured(
        &client.tools_call(verb, args).await.unwrap_or_else(|e| panic!("{verb}: {} {}", e.code, e.message)),
    )
}

#[tokio::test]
async fn trm_ac07_a_sub_key_reads_the_caller_view_of_its_tool_and_is_denied_the_rest() {
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "TRM AC7 Tenant").await;
    let primary = McpClient::with_bearer(&server.base_url, &key);

    ok(&primary, "host.secret.set", json!({"name": "ledger_token", "value": "trm-ac7-secret-value"})).await;
    for name in ["tool_a", "tool_b"] {
        ok(
            &primary,
            "host.tool.publish",
            json!({"name": name, "kind": "http", "spec": {
                "method": "GET", "url": "https://ledger.example.com/v1/entries",
                "headers": {"Authorization": "Bearer {{ secret.ledger_token }}"}}}),
        )
        .await;
    }
    let minted = ok(&primary, "host.key.mint", json!({"tools": ["tool_a"], "label": "only-a"})).await;
    let sub = McpClient::with_bearer(&server.base_url, minted["key"].as_str().expect("minted key"));

    // `a`, by its local name and by its wire name: the caller view, no secret name.
    for name in ["tool_a".to_string(), format!("{ns}.tool_a")] {
        let answer = ok(&sub, "host.tool.reach", json!({"name": name})).await;
        assert_eq!(
            answer["reach"],
            json!({"hosts": ["ledger.example.com"], "dynamic_hosts": false, "network": "none", "secrets_count": 1}),
            "{name}: {answer}"
        );
        assert_eq!(answer["tool_version"], json!(1));
        assert!(!answer.to_string().contains("ledger_token"), "a sub-key sees no secret name: {answer}");
    }

    // `b` is outside the allowlist, under either spelling.
    for name in ["tool_b".to_string(), format!("{ns}.tool_b")] {
        let err = sub
            .tools_call("host.tool.reach", json!({"name": name}))
            .await
            .expect_err("a tool outside the allowlist must be refused");
        assert_eq!(err.error_code.as_deref(), Some("permission_denied"), "{name}: {err:?}");
        assert_eq!(err.data["allowed_tools"], json!(["tool_a"]), "{name}");
    }

    // The primary key still reads the whole manifest of the same tool.
    let own = ok(&primary, "host.tool.reach", json!({"name": "tool_a"})).await;
    assert_eq!(own["reach"]["secrets"], json!(["ledger_token"]));
}
