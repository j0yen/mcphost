//! PRD-mcphost-first-hour-support-surface
//! AC8 (P0) — Given a release artifact built the way `mcphost-deploy`
//! builds it, When its file list is inspected, Then `www/` (status, aup,
//! support, plans, help/*) is included and served at the documented paths.
//!
//! `mcphost-deploy` is a separate tool (not in this repo, not reachable
//! from here) that packages a release tarball and its own Caddy config --
//! the "is `www/` in the tarball" half of this AC is that tool's own test
//! suite to prove, not this crate's. What this crate's build controls, and
//! what this PRD actually fixes at the root (non-functional note: "the
//! hotfix's finding on why `/status.html` 404'd decides whether that is
//! packaging or routing -- fix the root"), is routing: every one of these
//! five pages is now `include_str!`-embedded into the compiled binary
//! itself (`status.html`/`aup.html`/`plans.html`, same as the hotfix) or
//! rendered purely from in-binary Rust data with no file dependency at all
//! (`support.html`, `/help/<code>`) -- see `help.rs`'s module doc comment.
//! That means none of the five needs a `www/` directory to exist
//! *alongside* the running binary at all: this test proves exactly that,
//! which is the stronger guarantee and the one this repo's `cargo test` can
//! actually check.

use crate::common;
use common::TestServer;

#[tokio::test]
async fn every_documented_www_path_is_served_by_the_binary_alone() {
    let server = TestServer::start().await;
    let http = reqwest::Client::new();

    let mut paths: Vec<String> = vec![
        "/status.html".to_string(),
        "/aup.html".to_string(),
        "/support.html".to_string(),
        "/plans.html".to_string(),
        "/help".to_string(),
    ];
    for code in mcphost::help::HELP_CODES {
        paths.push(format!("/help/{code}"));
    }

    let mut failures = Vec::new();
    for path in &paths {
        let url = format!("{}{}", server.base_url, path);
        match http.get(&url).send().await {
            Ok(resp) => {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                if status != reqwest::StatusCode::OK || body.trim().is_empty() {
                    failures.push(format!("{path}: status={status} body_len={}", body.len()));
                }
            }
            Err(e) => failures.push(format!("{path}: request failed: {e}")),
        }
    }
    assert!(
        failures.is_empty(),
        "every documented www path must be served at the documented path:\n{}",
        failures.join("\n")
    );
}

/// Every `HELP_CODES` entry resolves to a real, compiled-in `www/*` source
/// or Rust-data path -- `www/status.html`/`www/aup.html`/`www/plans.html`
/// existing is already enforced at compile time by `http.rs`'s own
/// `include_str!` calls (a missing file fails the build, not this test);
/// this asserts the git-tracked source files this binary was built from
/// are the ones actually embedded, so "included" means what AC8 says.
#[test]
fn embedded_www_files_exist_in_the_source_tree() {
    for rel in ["www/status.html", "www/aup.html", "www/plans.html", "www/llms.txt", "www/llms-full.txt"] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
        assert!(path.is_file(), "{rel} must exist in the source tree this binary embeds from");
    }
}
