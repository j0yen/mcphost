//! PRD-mcphost-index-create-from-sources
//! AC3 -- Given `sources: [{kind: "blob"}]`, When `host.index.create` runs,
//! Then `args_invalid` is returned and its message lists exactly the
//! `IndexSourceKind` variants; Given the rendered schema and
//! `docs/retrieval.md`, Then the drift test asserts both equal the enum.

use crate::common;
use mcphost::errors::AppError;
use mcphost::index::{self, IndexSourceKind};
use mcphost::kinds::KindRegistry;
use serde_json::json;

fn variants() -> Vec<String> {
    IndexSourceKind::ALL.iter().map(|k| k.as_str().to_string()).collect()
}

fn scratch_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-hidx-ac03-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

#[tokio::test]
async fn an_unknown_source_kind_is_args_invalid_listing_exactly_the_variants() {
    let dir = scratch_dir();
    let state = common::bare_state(&dir).await;
    let tenant = common::bare_tenant(&state, "hidx-ac03").await;

    let err = index::index_create(&state, &tenant, &json!({"name": "bad", "sources": [{"kind": "blob"}]}))
        .await
        .expect_err("a blob source must be refused");
    assert_eq!(err.code(), "args_invalid");
    let AppError::InvalidArgs(message) = err else { panic!("expected InvalidArgs, got {err:?}") };
    let (_, listed) = message.split_once("one of: ").expect("message lists the kinds after 'one of: '");
    let listed: Vec<String> = listed.split(", ").map(str::to_string).collect();
    assert_eq!(listed, variants(), "the message lists exactly the enum's variants: {message}");
    assert!(!message.contains("blob"), "the offending value is not echoed as a kind: {message}");

    std::fs::remove_dir_all(&dir).ok();
}

/// The names between `<!-- kinds:start -->` and `<!-- kinds:end -->` in
/// `docs/retrieval.md`, one "- `name`: ..." line each.
fn doc_kinds() -> Vec<String> {
    let doc = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/retrieval.md")).expect("docs/retrieval.md");
    let block = doc
        .split("<!-- kinds:start -->")
        .nth(1)
        .and_then(|rest| rest.split("<!-- kinds:end -->").next())
        .expect("kinds block in docs/retrieval.md");
    block
        .lines()
        .filter_map(|l| l.strip_prefix("- `"))
        .filter_map(|l| l.split('`').next())
        .map(str::to_string)
        .collect()
}

#[test]
fn schema_enum_and_retrieval_doc_equal_the_enum() {
    let kinds = KindRegistry::with_builtin();
    let tools = mcphost::handler::host_tool_descriptors(&kinds);
    let tool = tools.iter().find(|t| t.name == "host.index.create").expect("host.index.create is registered");
    let rendered: Vec<String> = tool.input_schema["properties"]["sources"]["items"]["properties"]["kind"]["enum"]
        .as_array()
        .expect("sources[].kind carries an enum")
        .iter()
        .map(|v| v.as_str().expect("enum entries are strings").to_string())
        .collect();
    assert_eq!(rendered, variants(), "the tool schema's kind enum equals IndexSourceKind");
    assert_eq!(doc_kinds(), variants(), "docs/retrieval.md lists exactly IndexSourceKind");

    let llms = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/www/llms.txt")).expect("www/llms.txt");
    assert!(
        llms.contains(&format!("sources[].kind is one of: {}.", variants().join(", "))),
        "www/llms.txt names exactly IndexSourceKind"
    );
}
