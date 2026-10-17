//! PRD-mcphost-registry-entry-one-source AC6 (P0) -- Given `README.md` and
//! `docs/launch/wave1-checklist.md`, When the link test runs, Then both
//! contain `REGISTRY_SEARCH_URL` (the const's value, verbatim) and no other
//! `registry.modelcontextprotocol.io` URL.

use mcphost::registry_manifest::REGISTRY_SEARCH_URL;

fn registry_urls(text: &str) -> Vec<String> {
    let needle = "https://registry.modelcontextprotocol.io";
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(pos) = rest.find(needle) {
        let tail = &rest[pos..];
        let end = tail
            .find(|c: char| c.is_whitespace() || matches!(c, ')' | '\'' | '`' | '"' | '|' | ','))
            .unwrap_or(tail.len());
        out.push(tail[..end].trim_end_matches('.').to_string());
        rest = &tail[end..];
    }
    out
}

#[test]
fn regone_ac06() {
    for rel in ["README.md", "docs/launch/wave1-checklist.md"] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(REGISTRY_SEARCH_URL), "{rel} must contain REGISTRY_SEARCH_URL");
        for url in registry_urls(&text) {
            assert_eq!(url, REGISTRY_SEARCH_URL, "{rel} carries another registry URL");
        }
    }
    let items = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("launch/items.toml"),
    )
    .unwrap();
    for url in registry_urls(&items) {
        assert_eq!(url, REGISTRY_SEARCH_URL, "launch/items.toml carries another registry URL");
    }
}
