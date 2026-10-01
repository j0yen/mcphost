//! AC3 (P0) — Given a spec key one edit away from a known field (`sourc`),
//! When published, Then `data.did_you_mean == "source"`.

use crate::common;
use common::{TempDataDir, TestServer, all_kinds_registry, signup};
use serde_json::json;

#[tokio::test]
async fn a_one_edit_typo_gets_named_as_did_you_mean() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(all_kinds_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Unkfield AC3").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "typo",
                "kind": "python",
                "spec": {
                    // "sourc" (missing trailing "e") has no `source` key at
                    // all, so this also exercises python's own "source: is
                    // required" path NOT firing first -- the unknown-field
                    // check runs ahead of it.
                    "sourc": "def main(args):\n    return {\"ok\": True}\n",
                },
            }),
        )
        .await
        .expect_err("'sourc' is not a real field and must be refused");

    assert_eq!(err.error_code.as_deref(), Some("unknown_spec_field"));
    assert_eq!(err.data["field"], json!("sourc"));
    assert_eq!(err.data["did_you_mean"], json!("source"));
}

/// A key more than 2 edits from anything known gets no `did_you_mean` at
/// all, rather than a misleading distant guess.
#[tokio::test]
async fn a_far_miss_gets_no_did_you_mean() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(all_kinds_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Unkfield AC3b").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "farmiss",
                "kind": "python",
                "spec": {
                    "source": "def main(args):\n    return {\"ok\": True}\n",
                    "zzzzzzzzzz": 1,
                },
            }),
        )
        .await
        .expect_err("zzzzzzzzzz is not a real field and must be refused");

    assert_eq!(err.error_code.as_deref(), Some("unknown_spec_field"));
    assert!(
        err.data.get("did_you_mean").is_none(),
        "a field with no close match must carry no did_you_mean: {:?}",
        err.data
    );
}
