//! AC6 (PRD-mcphost-install-links-more-clients) — Given the README
//! generated block and `www/llms.txt`, When one row's artefact changes in
//! source, Then the docs check fails with a diff; after regeneration it
//! passes.
//!
//! "One row's artefact changes in source" is simulated by rendering the
//! table over a different base URL (every artefact changes), which is the
//! same code path a single-row edit takes through `splice_install_links_section`.

use mcphost::install_links::{self, CANONICAL_PUBLIC_URL};

const README: &str = include_str!("../README.md");
const LLMS_TXT: &str = include_str!("../www/llms.txt");

fn first_differing_line(a: &str, b: &str) -> Option<(String, String)> {
    a.lines().zip(b.lines()).find(|(x, y)| x != y).map(|(x, y)| (x.to_string(), y.to_string()))
}

#[test]
fn both_docs_carry_every_surface_and_match_a_fresh_render() {
    for (name, doc) in [("README.md", README), ("www/llms.txt", LLMS_TXT)] {
        assert_eq!(
            install_links::splice_install_links_section(doc, CANONICAL_PUBLIC_URL),
            doc,
            "{name}'s install-links block has drifted -- run `mcphost gen-docs`"
        );
        for s in &install_links::for_url(CANONICAL_PUBLIC_URL) {
            assert!(doc.contains(&s.artefact), "{name} is missing the {} artefact", s.id);
        }
    }
}

#[test]
fn a_changed_artefact_fails_the_check_with_a_diff_and_regeneration_fixes_it() {
    for (name, doc) in [("README.md", README), ("www/llms.txt", LLMS_TXT)] {
        let drifted = install_links::splice_install_links_section(doc, "https://changed.example");
        assert_ne!(drifted, doc, "{name}: a changed artefact must make the check fail");
        let (committed_line, fresh_line) =
            first_differing_line(doc, &drifted).expect("the check must be able to show a differing line");
        assert_ne!(committed_line, fresh_line, "{name}: diff shows the stale line");

        let regenerated = install_links::splice_install_links_section(&drifted, CANONICAL_PUBLIC_URL);
        assert_eq!(regenerated, doc, "{name}: regeneration must restore a passing state");
    }
}

#[test]
fn gen_docs_check_covers_both_docs() {
    let kinds = mcphost::kinds::KindRegistry::with_builtin();
    assert!(!mcphost::gendocs::run(true, &kinds).expect("gen-docs --check"), "gen-docs --check reports stale docs");
}
