//! PRD-mcphost-spec-unknown-field-rejection
//! AC3 — Given a spec key one edit away from a known field (`sourc`), When
//! published, Then `data.did_you_mean == "source"`.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

fn server_data_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-unkfield-ac03-{}-{}",
        std::process::id(),
        mcphost::state::now_unix()
    ));
    std::fs::create_dir_all(&dir).expect("create temp data dir");
    dir
}

#[tokio::test]
async fn near_miss_field_name_suggests_the_real_one() {
    let server = TestServer::start_with_kinds(common::python_kind_registry(
        server_data_dir().as_path(),
    ))
    .await;
    let (_ns, key) = signup(&server.base_url, "Unkfield AC3").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "hello",
                "kind": "python",
                "spec": {
                    "source": "def main(args):\n    return args\n",
                    "sourc": "def main(args):\n    return args\n",
                },
            }),
        )
        .await
        .expect_err("a typo'd spec field must fail");

    assert_eq!(err.error_code.as_deref(), Some("unknown_spec_field"));
    assert_eq!(err.data.get("field"), Some(&json!("sourc")));
    assert_eq!(err.data.get("did_you_mean"), Some(&json!("source")));
}

#[tokio::test]
async fn a_far_miss_field_name_carries_no_did_you_mean() {
    let server = TestServer::start_with_kinds(common::python_kind_registry(
        server_data_dir().as_path(),
    ))
    .await;
    let (_ns, key) = signup(&server.base_url, "Unkfield AC3 far miss").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "hello",
                "kind": "python",
                "spec": {
                    "source": "def main(args):\n    return args\n",
                    "write_table": "runs",
                },
            }),
        )
        .await
        .expect_err("an unknown spec field must fail");

    assert_eq!(err.error_code.as_deref(), Some("unknown_spec_field"));
    assert_eq!(
        err.data.get("did_you_mean"),
        None,
        "'write_table' is nowhere near any python field; got {:?}",
        err.data.get("did_you_mean")
    );
}
