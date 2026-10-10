//! AC5 — Given every hint in the table, When a test extracts tokens
//! matching `host\.[a-z_.]+`, Then each is a tool name in
//! `host_tools(kinds, true)`.

use mcphost::handler::llms_txt_tool_names;
use mcphost::kinds::http::UPSTREAM_REMEDIES;

/// Every `host.<[a-z_.]+>` token in `text` (hand-rolled: no regex dev-dep
/// assumption), with a sentence-ending `.` trimmed.
fn host_tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("host.") {
        let preceded_by_word = rest[..i].chars().next_back().is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
        let tail = &rest[i..];
        let len = tail
            .char_indices()
            .find(|(_, c)| !(c.is_ascii_lowercase() || *c == '_' || *c == '.'))
            .map_or(tail.len(), |(n, _)| n);
        if !preceded_by_word {
            out.push(tail[..len].trim_end_matches('.').to_string());
        }
        rest = &tail[len.max(1)..];
    }
    out
}

#[test]
fn every_tool_named_in_a_hint_exists() {
    let names = llms_txt_tool_names(&mcphost::kinds::KindRegistry::default());
    let mut seen = 0;
    for row in UPSTREAM_REMEDIES {
        for token in host_tokens(row.hint) {
            seen += 1;
            assert!(
                names.contains(&token),
                "hint for {} names `{token}`, which is not a host tool",
                row.code
            );
        }
    }
    assert!(seen >= 2, "expected at least secret.set and tool_call tokens, saw {seen}");
}
