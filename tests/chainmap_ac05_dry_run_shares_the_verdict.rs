//! PRD-mcphost-chain-near-miss-mapping-refused AC5 (P0) -- Given the AC 1
//! chain, When `host.tool_test` dry-runs it, Then the verdict is `will_fail`
//! with the same `{step, path, reason, did_you_mean}` entry as AC 1 and
//! `success` is not `true`, and the `verdict` and `dry_run_report` paths share
//! one `steps_verdict` call (no second implementation).

use crate::common;
use common::{McpClient, TempDataDir, TestServer, chain_and_python_kind_registry, extract_structured, publish, signup};
use mcphost::sandbox;
use serde_json::{Value, json};

const FETCH: &str = "def main(args):\n    return {\"rows\": [{\"id\": 1, \"value\": 2}]}\n";
const TRANSFORM: &str = "def main(args):\n    return {\"total\": sum(r[\"value\"] for r in args[\"rows\"])}\n";

fn chain_spec() -> Value {
    json!({"steps": [
        {"tool": "fetch_data", "args": {}},
        {"tool": "transform_data", "args": {"rows": {"from": "steps.0.rows"}}},
    ]})
}

fn entry(v: &Value) -> Value {
    json!({"step": v["step"], "path": v["path"], "reason": v["reason"], "did_you_mean": v["did_you_mean"]})
}

#[tokio::test]
async fn tool_test_reports_the_same_entry_as_publish_and_is_not_a_success() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs = TempDataDir::new();
    let server = TestServer::start_with_kinds(chain_and_python_kind_registry(&envs.0)).await;
    let (_ns, key) = signup(&server.base_url, "Chainmap AC5 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    publish(&client, "fetch_data", "python", json!({"source": FETCH})).await;
    publish(&client, "transform_data", "python", json!({"source": TRANSFORM})).await;

    let refused = client
        .tools_call("host.tool_publish", json!({"name": "daily_pipeline", "kind": "chain", "spec": chain_spec()}))
        .await
        .expect_err("publish refuses the chain");
    let published_entry = entry(&refused.data["failures"][0]);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "daily_pipeline", "kind": "chain", "spec": chain_spec(), "force": true}),
        )
        .await
        .expect("forced publish");
    let tested = extract_structured(
        &client
            .tools_call("host.tool_test", json!({"name": "daily_pipeline", "args": {}}))
            .await
            .expect("host.tool_test reports, it does not error"),
    );

    assert_eq!(tested["verdict"], json!("will_fail"), "{tested}");
    assert_ne!(tested["success"], json!(true), "{tested}");
    let failures = tested["evidence"].as_array().expect("evidence array");
    assert_eq!(failures.len(), 1, "{tested}");
    assert_eq!(entry(&failures[0]), published_entry, "{tested}");
    assert_eq!(
        published_entry,
        json!({"step": 2, "path": "steps.0.rows", "reason": "not a path", "did_you_mean": "$.steps[0].result.rows"})
    );
}

/// The text of `fn <name>` up to its closing brace.
fn function_body(text: &str, name: &str) -> String {
    let at = text
        .find(&format!("\nasync fn {name}("))
        .or_else(|| text.find(&format!("\npub(crate) async fn {name}(")))
        .or_else(|| text.find(&format!("\nfn {name}(")))
        .unwrap_or_else(|| panic!("fn {name} not found"));
    let rest = &text[at + 1..];
    let open = rest.find('{').expect("body");
    let mut depth = 0usize;
    for (i, c) in rest[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return rest[..open + i + 1].to_string();
                }
            }
            _ => {}
        }
    }
    panic!("unbalanced braces in fn {name}")
}

#[test]
fn verdict_and_dry_run_report_share_one_steps_verdict_call() {
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/kinds/chain.rs")).expect("read chain.rs");
    for caller in ["verdict", "dry_run_report"] {
        let body = function_body(&text, caller);
        assert_eq!(body.matches("steps_verdict(").count(), 1, "{caller} calls steps_verdict once");
        assert!(!body.contains("near_miss("), "{caller} has no second implementation of the check");
    }
    // The near-miss check is reached from `steps_verdict` alone.
    assert_eq!(text.matches("near_miss(").count(), 2, "one definition, one caller (steps_verdict)");
    assert!(function_body(&text, "steps_verdict").contains("near_miss("));
}
