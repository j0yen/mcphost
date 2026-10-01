//! AC2 (P0) — Given a python spec containing `url` (an http field), When
//! published, Then `data.valid_for` contains `"http"`.
//!
//! `{source, url}` is deliberately ambiguous to `infer::infer_kind_signal`
//! (both a python signal and an http signal are present, so it reports no
//! signal at all -- see `infer_kind_signal`'s `(true, true) => None` arm),
//! so this reaches `PythonKind::validate_all`'s new unknown-field check
//! rather than being refused earlier as a `kind_mismatch`.

use crate::common;
use common::{TempDataDir, TestServer, all_kinds_registry, signup};
use serde_json::json;

#[tokio::test]
async fn an_http_only_field_on_a_python_spec_names_http_as_valid_for() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(all_kinds_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Unkfield AC2").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "misplaced",
                "kind": "python",
                "spec": {
                    "source": "def main(args):\n    return {\"ok\": True}\n",
                    "url": "https://api.example.com/items",
                },
            }),
        )
        .await
        .expect_err("url is not a python field and must be refused");

    assert_eq!(err.error_code.as_deref(), Some("unknown_spec_field"));
    assert_eq!(err.data["field"], json!("url"));
    let valid_for = err.data["valid_for"].as_array().expect("valid_for is an array");
    assert!(
        valid_for.contains(&json!("http")),
        "valid_for must name http: {valid_for:?}"
    );
    assert!(
        !valid_for.iter().any(|v| v == "python"),
        "valid_for must never name the kind that just rejected the field: {valid_for:?}"
    );
}

/// Same check through `host.spec_test`, which must share the enrichment
/// (`enrich_unknown_spec_field_valid_for`) rather than only `host.tool_publish`
/// getting it.
#[tokio::test]
async fn spec_test_also_names_valid_for() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(all_kinds_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Unkfield AC2b").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call(
            "host.spec_test",
            json!({
                "kind": "python",
                "spec": {
                    "source": "def main(args):\n    return {\"ok\": True}\n",
                    "url": "https://api.example.com/items",
                },
                "invocations": [{}],
            }),
        )
        .await
        .expect_err("host.spec_test must refuse too");

    let valid_for = err.data["valid_for"].as_array().expect("valid_for is an array");
    assert!(valid_for.contains(&json!("http")));
}
