//! AC3 (PRD-mcphost-llms-install-doc) — Given the committed root
//! `llms-install.md`, When one surface artefact changes in source, Then the
//! docs check fails naming the file; after regeneration it passes and the
//! served and committed bytes are equal.
//!
//! "One artefact changes in source" is simulated by rendering over a
//! different base URL, the same path a single-row edit takes through
//! `render_install_doc`.

use crate::common;
use common::TestServer;
use mcphost::gendocs;
use mcphost::install_links::{self, CANONICAL_PUBLIC_URL};
use mcphost::kinds::KindRegistry;

const COMMITTED: &str = include_str!("../llms-install.md");

#[test]
fn a_source_change_fails_the_check_naming_the_file_and_regeneration_passes() {
    let err = gendocs::check_install_doc(COMMITTED, "https://changed.example").expect_err("drift must fail");
    assert!(err.contains("llms-install.md"), "message must name the file: {err}");
    assert_eq!(gendocs::check_install_doc(COMMITTED, CANONICAL_PUBLIC_URL), Ok(()), "committed copy is current");
    assert_eq!(COMMITTED, install_links::render_install_doc(CANONICAL_PUBLIC_URL));
}

#[test]
fn gen_docs_check_covers_the_file_and_is_not_stale() {
    let stale = gendocs::run(true, &KindRegistry::with_builtin()).expect("gen-docs --check");
    assert!(!stale, "mcphost gen-docs --check reports stale docs -- run `mcphost gen-docs`");
}

#[tokio::test]
async fn served_and_committed_bytes_are_equal_modulo_the_server_hostname() {
    let server = TestServer::start().await;
    let served = reqwest::get(format!("{}/llms-install.md", server.base_url))
        .await
        .expect("GET")
        .text()
        .await
        .expect("body");
    let host = server.state.public_url.trim_end_matches('/');
    assert_ne!(host, CANONICAL_PUBLIC_URL);
    // Deep links embed the hostname base64-/percent-encoded, so a plain
    // replace cannot map them; every other line must match byte for byte.
    let plain = |doc: &str| {
        doc.lines()
            .filter(|l| !l.starts_with("cursor://") && !l.starts_with("vscode:mcp/"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert_eq!(plain(&served.replace(host, CANONICAL_PUBLIC_URL)), plain(COMMITTED));
    assert_eq!(served, install_links::render_install_doc(host), "served = generator output over its own host");
}
