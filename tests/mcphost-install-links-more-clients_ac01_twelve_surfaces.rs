//! AC1 (PRD-mcphost-install-links-more-clients) — Given
//! `MCPHOST_PUBLIC_URL=https://mcphost.dev`, When `install_links::for_url`
//! runs, Then it returns 12 surfaces with the ids listed in requirement 1,
//! each with non-empty `artefact`, an `https://` `doc_url`, and a `checked`
//! date.

use mcphost::install_links;

const EXPECTED_IDS: [&str; 13] = [
    "claude_code",
    "cursor",
    "vscode",
    "claude_ai",
    "chatgpt",
    "codex_cli",
    "gemini_cli",
    "opencode",
    "amp",
    "goose",
    "warp",
    "windsurf",
    "cline",
];

#[test]
fn for_url_returns_the_twelve_surfaces_with_complete_rows() {
    let surfaces = install_links::for_url("https://mcphost.dev");
    let ids: Vec<&str> = surfaces.iter().map(|s| s.id).collect();
    assert_eq!(ids, EXPECTED_IDS, "{surfaces:?}");
    assert_eq!(surfaces.len(), 13); // PRD-mcphost-chatgpt-submission-pack added `chatgpt`

    for s in &surfaces {
        assert!(!s.artefact.trim().is_empty(), "{} has an empty artefact", s.id);
        assert!(!s.label.is_empty(), "{} has no label", s.id);
        assert!(s.doc_url.starts_with("https://"), "{} doc_url {:?}", s.id, s.doc_url);
        // `checked` is an ISO date: YYYY-MM-DD.
        let parts: Vec<&str> = s.checked.split('-').collect();
        assert!(
            parts.len() == 3
                && parts[0].len() == 4
                && parts[1].len() == 2
                && parts[2].len() == 2
                && parts.iter().all(|p| p.chars().all(|c| c.is_ascii_digit())),
            "{} checked {:?} is not a YYYY-MM-DD date",
            s.id,
            s.checked
        );
    }
}
