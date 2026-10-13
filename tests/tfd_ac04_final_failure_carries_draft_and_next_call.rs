//! AC4 (P0) -- Given two failing drafts, When the call completes, Then the
//! error is the last `spec_test` error unchanged plus `data.draft_spec` and
//! `next_call.tool == "host.tool_publish"` whose `args.spec` equals
//! `draft_spec`.

use crate::common;
use common::{McpClient, TestServer, python_kind_registry, signup};
use mcphost::drafter::{Draft, FakeDrafter};
use mcphost::sandbox;
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn two_failing_drafts_return_last_error_with_draft_spec_and_publish_next_call() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let first = Draft {
        name: Some("broken".into()),
        spec: json!({"source": "def main(args):\n    return {\"v\": first_undefined}\n"}),
        invocations: vec![json!({})],
        ..Draft::default()
    };
    let last_spec = json!({"source": "def main(args):\n    return {\"v\": last_undefined}\n"});
    let second = Draft {
        name: Some("broken".into()),
        spec: last_spec.clone(),
        invocations: vec![json!({})],
        ..Draft::default()
    };
    let fake = Arc::new(FakeDrafter::new(vec![first, second]));
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_drafter(python_kind_registry(&envs_dir.0), fake.clone()).await;
    let (_ns, key) = signup(&server.base_url, "TFD AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    // The last draft's failure as `host.spec_test` itself reports it.
    let direct = client
        .tools_call("host.spec_test", json!({"kind": "python", "spec": last_spec, "invocations": [{}]}))
        .await
        .expect("spec_test reports invocation failures inside a successful response");
    let direct = common::extract_structured(&direct);
    let expected = &direct["invocations"][0];
    assert_eq!(expected["ok"], json!(false), "{direct}");

    let err = client
        .tools_call("host.tool_from_description", json!({"description": "never works"}))
        .await
        .expect_err("two failing drafts is an error");
    assert_ne!(err.error_code.as_deref(), Some("internal"), "never a generic internal: {err:?}");
    assert_eq!(fake.prompts().len(), 2, "exactly one retry");

    assert_eq!(err.data["draft_spec"], last_spec, "{:?}", err.data);
    let next = &err.data["next_call"];
    assert_eq!(next["tool"], json!("host.tool_publish"), "{:?}", err.data);
    assert_eq!(next["args"]["spec"], err.data["draft_spec"]);
    // The registry's own failure for the last draft is carried unchanged.
    assert_eq!(err.data["invocation"]["code"], expected["code"], "{:?}", err.data);
    assert_eq!(err.data["invocation"]["message"], expected["message"], "{:?}", err.data);
    assert_eq!(err.data["invocation"]["exception_class"], expected["exception_class"], "{:?}", err.data);
}
