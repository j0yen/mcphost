//! PRD-mcphost-tool-test-examples-from-schema AC7 — `host.quickstart(kind:
//! python)` and `www/llms.txt` carry the same `try_before_call` sentence,
//! saying `args` may be omitted and naming `example_args`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, python_kind_registry, signup};
use mcphost::control::TRY_BEFORE_CALL_OMIT_ARGS;
use serde_json::json;

#[test]
fn the_sentence_says_args_may_be_omitted_and_names_example_args() {
    assert!(TRY_BEFORE_CALL_OMIT_ARGS.contains("`args` may be omitted"));
    assert!(TRY_BEFORE_CALL_OMIT_ARGS.contains("`example_args`"));
}

#[test]
fn llms_txt_carries_the_sentence() {
    let llms = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/www/llms.txt")).expect("www/llms.txt");
    assert!(llms.contains(TRY_BEFORE_CALL_OMIT_ARGS), "www/llms.txt is missing the try_before_call sentence");
}

#[tokio::test]
async fn quickstart_python_carries_the_same_sentence() {
    // The python kind is registered (not run): quickstart only describes it.
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Exargs AC7").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let out = extract_structured(&client.tools_call("host.quickstart", json!({"kind": "python"})).await.expect("quickstart"));
    let rows = out["try_before_call"].as_array().expect("try_before_call");
    assert!(
        rows.iter().any(|r| r["note"] == json!(TRY_BEFORE_CALL_OMIT_ARGS)),
        "no try_before_call row carries the sentence: {rows:?}"
    );
}
