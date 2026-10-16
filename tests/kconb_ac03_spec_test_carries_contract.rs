//! AC3 — Given `host.spec_test("python", spec, [])`, When it returns, Then
//! the response carries `contract.families` and `contract.source_line: true`.

use crate::common;
use common::{TestServer, extract_structured, python_kind_registry, signup};
use mcphost::kinds::KindRegistry;
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn kconb_ac03_python_spec_test_carries_contract() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Kconb AC3 Python").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let result = client
        .tools_call(
            "host.spec_test",
            json!({
                "kind": "python",
                "spec": {"source": "def main(args):\n    return {'ok': True}\n"},
                "invocations": [],
            }),
        )
        .await
        .expect("host.spec_test on a python spec with no invocations succeeds");
    let structured = extract_structured(&result);
    let families = structured["contract"]["families"].as_array().expect("contract.families");
    assert!(families.iter().any(|f| f == "tool_code"), "python lists tool_code: {structured}");
    assert_eq!(structured["contract"]["source_line"], json!(true));
}

#[tokio::test]
async fn kconb_ac03_echo_spec_test_contract_has_no_source_line() {
    let server = TestServer::start_with_kinds(KindRegistry::with_builtin()).await;
    let (_ns, key) = signup(&server.base_url, "Kconb AC3 Echo").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);
    let result = client
        .tools_call(
            "host.spec_test",
            json!({"kind": "echo", "spec": {"schema": {"type": "object"}}, "invocations": []}),
        )
        .await
        .expect("host.spec_test on echo succeeds");
    let structured = extract_structured(&result);
    assert_eq!(structured["contract"]["source_line"], json!(false));
    assert!(structured["contract"]["families"].is_array());
}
