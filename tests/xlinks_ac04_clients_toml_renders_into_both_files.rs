//! AC4 (PRD-mcphost-docs-external-links-resolve) -- Given
//! `docs/clients.toml`, When `mcphost llms-txt` runs, Then README.md and
//! www/llms.txt each contain the rendered block between
//! `<!-- clients:start -->` and `<!-- clients:end -->`, and the drift test
//! finds the toml URL set equal to both blocks' URL sets.

use mcphost::clients;

use crate::xlinks_cli;

const README: &str = include_str!("../README.md");
const LLMS_TXT: &str = include_str!("../www/llms.txt");

#[test]
fn llms_txt_command_renders_the_block_into_both_files() {
    let s = xlinks_cli::Scratch::new("ac04");
    let stale = |doc: String| {
        let (a, b) = (doc.find(clients::CLIENTS_SECTION_START).unwrap(), doc.find(clients::CLIENTS_SECTION_END).unwrap());
        format!("{}{}\nstale\n{}{}", &doc[..a], clients::CLIENTS_SECTION_START, clients::CLIENTS_SECTION_END, &doc[b + clients::CLIENTS_SECTION_END.len()..])
    };
    s.write("README.md", &stale(s.read("README.md")));
    s.write("llms.txt", &stale(s.read("llms.txt")));

    let out = s.llms_txt(false);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let want = clients::render_block(clients::builtin());
    for name in ["README.md", "llms.txt"] {
        assert_eq!(clients::block_of(&s.read(name)), Some(want.as_str()), "{name}");
    }
    let again = s.llms_txt(true);
    assert!(again.status.success(), "{}", String::from_utf8_lossy(&again.stderr));
}

#[test]
fn committed_files_carry_the_render() {
    let want = clients::render_block(clients::builtin());
    for (name, doc) in [("README.md", README), ("www/llms.txt", LLMS_TXT)] {
        assert_eq!(clients::block_of(doc), Some(want.as_str()), "{name} differs from the render -- run `mcphost llms-txt`");
    }
}

#[test]
fn toml_docs_urls_equal_the_external_urls_in_both_blocks() {
    let toml_urls: std::collections::BTreeSet<String> =
        clients::builtin().client.iter().map(|c| c.docs_url.clone()).collect();
    assert_eq!(toml_urls.len(), 13, "one docs_url per client");
    for (name, doc) in [("README.md", README), ("www/llms.txt", LLMS_TXT)] {
        let block = clients::block_of(doc).unwrap_or_else(|| panic!("{name} has no clients block"));
        assert_eq!(clients::external_urls(block), toml_urls, "{name}'s block URL set differs from docs/clients.toml");
    }
}

#[test]
fn toml_install_text_is_what_install_links_builds() {
    // The toml's `install` field is data; `install_links` builds the same
    // text from code for `/connect` and `host.quickstart`. They must not
    // drift: the gen-docs splice over the committed files is a no-op.
    for (name, doc) in [("README.md", README), ("www/llms.txt", LLMS_TXT)] {
        assert_eq!(
            mcphost::install_links::splice_install_links_section(doc, mcphost::install_links::CANONICAL_PUBLIC_URL),
            doc,
            "{name}: docs/clients.toml `install` text differs from src/install_links.rs"
        );
    }
}
