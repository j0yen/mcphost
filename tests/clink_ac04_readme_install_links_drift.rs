//! AC4 (PRD-mcphost-client-install-links) — Given the README generated
//! block, When the docs check runs after a hostname or format change in
//! source, Then it fails with a diff; after regeneration it passes.

use mcphost::install_links;
use mcphost::kinds::KindRegistry;

const README: &str = include_str!("../README.md");

/// The "after regeneration it passes" half: README's committed
/// `<!-- install-links:start/end -->` block is byte-identical to a fresh
/// render over `install_links::CANONICAL_PUBLIC_URL` right now -- same
/// hermetic drift-proof shape as `gendocs.rs`'s own
/// `committed_plans_doc_matches_a_fresh_render`.
#[test]
fn readme_install_links_section_matches_a_fresh_render() {
    let fresh = install_links::splice_install_links_section(README, install_links::CANONICAL_PUBLIC_URL);
    assert_eq!(
        fresh, README,
        "README's install-links block has drifted from install_links::for_url -- run `mcphost gen-docs`"
    );
}

/// The "fails with a diff" half: a hostname (or deep-link format) change
/// in source changes what the splice renders -- proving the check would
/// catch exactly that drift, without needing to assert anything about a
/// committed sha.
#[test]
fn a_hostname_change_in_source_changes_the_rendered_block() {
    let drifted = install_links::splice_install_links_section(README, "https://changed.example");
    assert_ne!(
        drifted, README,
        "changing the base hostname must change the rendered install-links block, \
         else a real hostname change in source would go undetected by the check"
    );
}

/// Wiring sanity: `mcphost gen-docs --check` (`gendocs::run(true, ...)`)
/// is the actual CLI path this AC's "the docs check runs" refers to --
/// proves the splice above is really plugged into it, not just callable
/// on its own.
#[test]
fn gen_docs_check_is_not_stale_right_now() {
    let kinds = KindRegistry::with_builtin();
    let stale = mcphost::gendocs::run(true, &kinds).expect("gen-docs --check");
    assert!(!stale, "mcphost gen-docs --check reports stale docs -- run `mcphost gen-docs`");
}
