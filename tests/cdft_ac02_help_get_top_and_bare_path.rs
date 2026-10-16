//! PRD-mcphost-chain-friction-fixes-20261016 AC2 (P0) -- `host.help.get`
//! resolves `<path>#top` and a bare `<path>` to the document's first
//! generated anchor; an unknown slug on a known path offers that path's
//! anchors first.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use mcphost::doc_anchors::{DocAnchor, ENTRIES};
use serde_json::json;

const PATH: &str = "docs/kinds/chain.md";

fn first_anchor() -> &'static str {
    ENTRIES.iter().find(|e| e.path == PATH).expect("chain.md has headings").anchor
}

#[tokio::test]
async fn top_and_bare_path_return_the_first_section() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);
    let first = ENTRIES.iter().find(|e| e.path == PATH).expect("entry");
    for anchor in [format!("{PATH}#top"), PATH.to_string()] {
        let result = client
            .tools_call("host.help.get", json!({"anchor": anchor}))
            .await
            .unwrap_or_else(|e| panic!("{anchor}: {e:?}"));
        let body = extract_structured(&result);
        assert_eq!(body["anchor"], first_anchor(), "{anchor}");
        assert_eq!(body["text"].as_str(), Some(first.text), "{anchor}");
        assert_eq!(body["path"], PATH);
    }
}

#[tokio::test]
async fn unknown_slug_on_a_known_path_lists_that_paths_anchors_first() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);
    let err = client
        .tools_call("host.help.get", json!({"anchor": format!("{PATH}#nope")}))
        .await
        .expect_err("unknown slug is refused");
    assert_eq!(err.error_code.as_deref(), Some("anchor_not_found"), "{err:?}");
    let near = err.data["did_you_mean"].as_array().expect("did_you_mean");
    assert!(!near.is_empty());
    let prefix = format!("{PATH}#");
    assert!(near[0].as_str().unwrap().starts_with(&prefix), "{near:?}");
}

#[test]
fn parse_unit_cases() {
    assert_eq!(DocAnchor::parse(&format!("{PATH}#top")).map(DocAnchor::as_str), Some(first_anchor()));
    assert_eq!(DocAnchor::parse(PATH).map(DocAnchor::as_str), Some(first_anchor()));
    assert!(DocAnchor::parse("docs/kinds/nonexistent.md#top").is_none());
    assert!(DocAnchor::parse(&format!("{PATH}#nope")).is_none());
}
