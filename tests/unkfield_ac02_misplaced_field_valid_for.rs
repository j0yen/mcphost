//! PRD-mcphost-spec-unknown-field-rejection
//! AC2 — Given a python spec containing `url` (an http field), When
//! published, Then `data.valid_for` contains `"http"`.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

fn server_data_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-unkfield-ac02-{}-{}",
        std::process::id(),
        mcphost::state::now_unix()
    ));
    std::fs::create_dir_all(&dir).expect("create temp data dir");
    dir
}

#[tokio::test]
async fn python_spec_carrying_an_http_field_names_valid_for_http() {
    let server =
        TestServer::start_with_kinds(common::all_kinds_registry(server_data_dir().as_path()))
            .await;
    let (_ns, key) = signup(&server.base_url, "Unkfield AC2").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "hello",
                "kind": "python",
                "spec": {
                    "source": "def main(args):\n    return args\n",
                    "url": "https://api.example.com",
                },
            }),
        )
        .await
        .expect_err("a python spec carrying an http-only field must fail");

    assert_eq!(err.error_code.as_deref(), Some("unknown_spec_field"));
    assert_eq!(err.data.get("field"), Some(&json!("url")));
    let valid_for = err
        .data
        .get("valid_for")
        .and_then(|v| v.as_array())
        .expect("data.valid_for is an array")
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert!(
        valid_for.contains(&"http".to_string()),
        "valid_for: {valid_for:?}"
    );
}
