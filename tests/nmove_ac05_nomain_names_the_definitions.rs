//! PRD-mcphost-refusals-name-the-nearest-move
//! AC5 (P0) -- Given `host.tool_publish` and `host.spec_test` with sources
//! defining `async def main(args)`, `def run(args)` and `main` only inside a
//! class, When each runs, Then each fails `invalid_spec` with
//! `exception_class` `NoMain`, `defs_found` `["async main"]`, `["run"]` and
//! `[]` respectively, and `fix` naming `def main(args)` at module top level,
//! while `message` stays `source does not define main(args)`.

use crate::common;
use common::{McpClient, TestServer, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn nmove_ac05() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Nmove05").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let cases = [
        ("async def main(args):\n    return args\n", json!(["async main"])),
        ("def run(args):\n    return args\n", json!(["run"])),
        ("class Tool:\n    def main(self, args):\n        return args\n", json!([])),
    ];
    for (i, (source, defs)) in cases.into_iter().enumerate() {
        let spec = json!({"source": source, "args_schema": {"type": "object"}});
        let publish = client
            .tools_call("host.tool_publish", json!({"name": format!("nomain_{i}"), "kind": "python", "spec": spec.clone()}))
            .await
            .expect_err("publish without a top-level def main is refused");
        let test = client
            .tools_call("host.spec_test", json!({"kind": "python", "spec": spec, "invocations": [{}]}))
            .await
            .expect_err("spec_test refuses it identically");
        for (verb, err) in [("host.tool_publish", &publish), ("host.spec_test", &test)] {
            assert_eq!(err.error_code.as_deref(), Some("invalid_spec"), "{verb} case {i}: {err:?}");
            assert!(err.message.contains("source does not define main(args)"), "{verb} case {i}: {}", err.message);
            assert_eq!(err.data["exception_class"], "NoMain", "{verb} case {i}: {err:?}");
            assert_eq!(err.data["defs_found"], defs, "{verb} case {i}: {err:?}");
            let fix = err.data["fix"].as_str().unwrap_or_default();
            assert!(fix.contains("def main(args)") && fix.contains("module top level"), "{verb} case {i}: {fix}");
        }
        assert_eq!(publish.data["defs_found"], test.data["defs_found"]);
        assert_eq!(publish.data["fix"], test.data["fix"]);
    }
}
