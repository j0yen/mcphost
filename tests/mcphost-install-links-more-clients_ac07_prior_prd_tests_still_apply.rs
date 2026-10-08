//! AC7 (PRD-mcphost-install-links-more-clients) — Given
//! PRD-mcphost-client-install-links' test files, When run against this
//! build, Then they pass unchanged.
//!
//! The `clink_ac01..06` files themselves run in the same suite binary as
//! this one (`scripts/gen-test-suites.sh` lists them); this test proves
//! they are still present, still registered in a suite, and that the
//! compatibility view they exercise (`Links`' original fields) is a
//! faithful projection of the new table.

use mcphost::install_links;

const SUITE: &str = include_str!("suite_core_07.rs");

#[test]
fn the_prior_prd_test_files_are_still_registered_in_a_suite() {
    for file in [
        "clink_ac01_for_url_pinned_fixtures.rs",
        "clink_ac02_connect_page_unauthenticated.rs",
        "clink_ac03_quickstart_personal_install_links.rs",
        "clink_ac04_readme_install_links_drift.rs",
        "clink_ac05_connect_go_records_click_and_redirects.rs",
        "clink_ac06_digest_install_link_clicks_humans_only.rs",
    ] {
        assert!(SUITE.contains(&format!("#[path = \"{file}\"]")), "{file} is no longer part of the suite");
    }
}

#[test]
fn the_original_four_forms_are_projections_of_the_table_rows() {
    let links = install_links::for_url("https://mcphost.dev");
    let artefact = |id: &str| links.iter().find(|s| s.id == id).expect(id).artefact.clone();
    assert_eq!(links.cursor, artefact("cursor"));
    assert_eq!(links.vscode, artefact("vscode"));
    assert_eq!(links.claude_code_command, artefact("claude_code"));
    assert_eq!(links.mcp_url, "https://mcphost.dev/mcp");
    let steps: Vec<String> = artefact("claude_ai")
        .lines()
        .map(|l| l.split_once(". ").expect("numbered").1.to_string())
        .collect();
    assert_eq!(links.claude_ai_steps, steps);
}
