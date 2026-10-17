//! PRD-mcphost-sandbox-file-processing-libs
//! AC7 (P1) -- Given the planted probe tool, When `host.spec_test` runs it,
//! Then it returns PDF text, 100 xlsx rows and a 256 px PNG within the
//! default timeout.

use crate::common;
use common::{TestServer, extract_structured, python_kind_registry, signup};
use mcphost::{sandbox, sandbox_libs};
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn sblibs_ac07_probe_tool_returns_pdf_text_100_rows_and_a_256px_png() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Sblibs AC7 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let mut out = json!(null);
    for _ in 0..200 {
        out = extract_structured(
            &client
                .tools_call(
                    "host.spec_test",
                    json!({"kind": "python", "spec": sandbox_libs::probe_spec(), "invocations": [{}]}),
                )
                .await
                .unwrap_or_else(|e| panic!("spec_test failed: {} {}", e.code, e.message)),
        );
        if out["invocations"][0]["status"] != json!("building") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let text = out.to_string();
    let result = &out["invocations"][0];
    assert!(result["ok"] != json!(false), "invocation failed: {text}");
    for expected in ["Alpha page one", "Bravo page two", "Charlie page three", "\"xlsx_rows\":100", "\"png_px\":256"] {
        assert!(text.contains(expected), "spec_test output must contain {expected}: {text}");
    }
    let in_bytes = text
        .split("\"png_in_bytes\":")
        .nth(1)
        .and_then(|r| r.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|n| n.parse::<u64>().ok())
        .expect("png_in_bytes");
    assert!(in_bytes >= 1_000_000, "the input PNG is about 1 MB: {in_bytes}");
}
