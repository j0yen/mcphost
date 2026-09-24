//! AC7 (PRD-mcphost-claude-code-plugin-and-snippets) — Given any file
//! under `plugin/` or the snippets, When grepped for a 40+ character
//! base64-like token following `Bearer `, Then only the literal `<key>`
//! placeholder appears.

const FILES: &[(&str, &str)] = &[
    (
        "plugin/.claude-plugin/plugin.json",
        include_str!("../plugin/.claude-plugin/plugin.json"),
    ),
    ("plugin/.mcp.json", include_str!("../plugin/.mcp.json")),
    (
        "plugin/skills/mcphost/SKILL.md",
        include_str!("../plugin/skills/mcphost/SKILL.md"),
    ),
    ("README.md", include_str!("../README.md")),
    ("www/index.html", include_str!("../www/index.html")),
    ("www/llms.txt", include_str!("../www/llms.txt")),
];

/// A base64-alphabet char (standard or URL-safe): the shape a real bearer
/// token would take, whichever alphabet it was minted with.
fn is_base64_like(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '-' | '_')
}

fn longest_base64_run_after_bearer(text: &str) -> Vec<(usize, usize)> {
    let mut hits = Vec::new();
    let mut search_from = 0;
    while let Some(rel) = text[search_from..].find("Bearer ") {
        let start = search_from + rel + "Bearer ".len();
        let run_len = text[start..]
            .chars()
            .take_while(|c| is_base64_like(*c))
            .count();
        if run_len >= 40 {
            hits.push((start, run_len));
        }
        search_from = start;
    }
    hits
}

#[test]
fn no_file_leaks_a_real_bearer_token() {
    for (path, content) in FILES {
        let hits = longest_base64_run_after_bearer(content);
        assert!(
            hits.is_empty(),
            "{path} has a 'Bearer <40+ char token>' that looks like a real key, not the \
             literal <key> placeholder: {hits:?}"
        );
    }
}

#[test]
fn placeholder_key_is_present_where_a_bearer_header_is_shown() {
    let has_placeholder = FILES
        .iter()
        .any(|(_, content)| content.contains("Bearer <key>") || content.contains("Bearer &lt;key&gt;"));
    assert!(
        has_placeholder,
        "at least one snippet must show the Cursor-style Bearer <key> placeholder"
    );
}
