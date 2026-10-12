//! PRD-mcphost-blob-put-get-signed-url
//! AC6 — Given a python tool `import mcphost` calling
//! `mcphost.blob.get(args["blob"])`, When the tool runs under
//! `network: none`, Then it receives the exact bytes; Given
//! `import mcphost.blobs`, Then the `unknown_import` hint lists
//! `mcphost.blob`.

use crate::common;
use base64::Engine as _;
use common::{McpClient, TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

fn fixture_pdf() -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/blobput/report.pdf");
    std::fs::read(path).expect("read report.pdf fixture")
}

#[tokio::test]
async fn python_tool_under_network_none_receives_the_exact_bytes() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Blobput AC6 Reader").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let pdf = fixture_pdf();
    let put = extract_structured(
        &client
            .tools_call(
                "host.blob.put",
                json!({"name": "report.pdf", "content_base64": base64::engine::general_purpose::STANDARD.encode(&pdf)}),
            )
            .await
            .expect("host.blob.put"),
    );
    let id = put["id"].as_str().expect("id").to_string();

    let spec = json!({
        "network": "none",
        "source": "import base64, hashlib, mcphost\ndef main(args):\n    data = mcphost.blob.get(args['blob'])\n    return {\"type\": type(data).__name__, \"len\": len(data), \"sha256\": hashlib.sha256(data).hexdigest(), \"b64\": base64.b64encode(data).decode()}\n",
        "args_schema": {"type": "object", "properties": {"blob": {"type": "string"}}, "required": ["blob"]},
    });
    client
        .tools_call("host.tool_publish", json!({"name": "blob_reader", "kind": "python", "spec": spec}))
        .await
        .expect("publish");

    let result = poll_until_ready(&client, &format!("{ns}.blob_reader"), json!({"blob": id}), Duration::from_secs(15))
        .await
        .unwrap_or_else(|e| panic!("call must succeed: {} {}", e.code, e.message));
    let out = extract_structured(&result);
    assert_eq!(out["type"], "bytes");
    assert_eq!(out["len"], pdf.len());
    assert_eq!(out["sha256"], mcphost::billing::sha256_hex(&pdf));
    let got = base64::engine::general_purpose::STANDARD.decode(out["b64"].as_str().unwrap()).unwrap();
    assert_eq!(got, pdf, "the tool received the exact bytes");

    // The same name works as the reference.
    let by_name = poll_until_ready(
        &client,
        &format!("{ns}.blob_reader"),
        json!({"blob": "report.pdf"}),
        Duration::from_secs(15),
    )
    .await
    .unwrap_or_else(|e| panic!("by-name call must succeed: {} {}", e.code, e.message));
    assert_eq!(extract_structured(&by_name)["sha256"], mcphost::billing::sha256_hex(&pdf));
}

#[tokio::test]
async fn python_tool_put_returns_a_url_that_serves_the_bytes() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Blobput AC6 Writer").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "import mcphost\ndef main(args):\n    return mcphost.blob.put('out.csv', b'a,b\\n1,2\\n')\n",
    });
    client
        .tools_call("host.tool_publish", json!({"name": "blob_writer", "kind": "python", "spec": spec}))
        .await
        .expect("publish");
    let result = poll_until_ready(&client, &format!("{ns}.blob_writer"), json!({}), Duration::from_secs(15))
        .await
        .unwrap_or_else(|e| panic!("call must succeed: {} {}", e.code, e.message));
    let out = extract_structured(&result);
    assert_eq!(out["mime"], "text/csv");
    assert_eq!(out["bytes"], 8);
    let resp = reqwest::get(out["url"].as_str().expect("url")).await.expect("GET url");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    assert_eq!(resp.text().await.unwrap(), "a,b\n1,2\n");
}

#[tokio::test]
async fn import_mcphost_blobs_is_unknown_and_the_hint_lists_mcphost_blob() {
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Blobput AC6 Hint").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "bad_blobs", "kind": "python", "spec": {"source": "import mcphost.blobs\ndef main(args):\n    return {}\n"}}),
        )
        .await
        .expect_err("mcphost.blobs is not a module");
    assert_eq!(err.data["error_code"], json!("unknown_import"));
    let hint = err.data["hint"].as_str().expect("data.hint");
    let listed = hint.split(['(', ')']).find(|part| part.contains("mcphost.state")).expect("module list in hint");
    let modules: Vec<&str> = listed.split(", ").collect();
    assert!(modules.contains(&"mcphost.blob"), "hint must list mcphost.blob: {hint}");
    assert!(!modules.contains(&"mcphost.blobs"), "mcphost.blobs is the typo, not a module: {hint}");
}

#[test]
fn bridge_registration_is_generated_not_hand_listed() {
    // R6: the hint list and the runner script's registration cannot drift.
    let registered = mcphost::kinds::python::runner_script_registered_modules();
    assert!(registered.iter().any(|m| m == "blob"), "{registered:?}");
    assert!(mcphost::kinds::python::bridge_module_list().contains("mcphost.blob"));
    assert!(mcphost::kinds::python::unknown_import_hint().contains("mcphost.blob"));
}
