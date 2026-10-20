//! PRD-mcphost-dev-emulator
//! AC1 (P0) -- Given `tests/fixtures/parity/type-error-14/` (raises at line
//! 14), When called through `mcphost dev --no-sandbox --call` and through
//! `TestServer::start` + `host.tool_call`, Then both envelopes carry
//! `code = tool_exception`, `data.line = 14` and byte-equal
//! `data.source_line`.

use crate::common;
use common::{McpClient, TestServer, python_kind_registry, tenant_url};
use mcphost::sandbox;
use serde_json::{Value, json};
use tokio::process::Command;

const FIXTURE: &str = "tests/fixtures/parity/type-error-14";

#[tokio::test]
async fn demu_ac01_type_error_14_is_the_same_envelope_locally_and_on_the_host() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE);

    // The emulator: one call, no server.
    let out = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .arg("dev")
        .arg(&fixture)
        .args(["--no-sandbox", "--call", "type_error_14", "{}"])
        .output()
        .await
        .expect("run mcphost dev --call");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "an error envelope exits 1: {stdout}");
    let local: Value = serde_json::from_str(stdout.trim()).expect("the envelope is one JSON object");
    let local = &local["error"]["data"];

    // The host: publish the same directory's payload, then host.tool_call.
    let data = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&data.0)).await;
    let url = tenant_url(&server.base_url).await;
    let path = url.trim_start_matches(&server.base_url).to_string();
    let client = McpClient::new(&server.base_url).with_path(&path);
    let payload = mcphost::tooldir::pack(&fixture).expect("pack the fixture");
    common::publish(
        &client,
        payload["name"].as_str().expect("name"),
        "python",
        payload["spec"].clone(),
    )
    .await;
    let err = client
        .tools_call("host.tool_call", json!({"name": "type_error_14", "args": {}}))
        .await
        .expect_err("the fixture raises");

    assert_eq!(local["error_code"], json!("tool_exception"), "{local}");
    assert_eq!(err.error_code.as_deref(), Some("tool_exception"), "{err:?}");
    assert_eq!(local["line"], json!(14), "{local}");
    assert_eq!(err.data["line"], json!(14), "{err:?}");
    assert_eq!(local["source_line"], err.data["source_line"], "byte-equal source_line");
    assert_eq!(local["source_line"], json!("return rows[\"name\"]"), "{local}");
}
