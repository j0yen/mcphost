//! PRD-mcphost-first-hour-support-surface
//! AC6 (P0) — Given README, docs and llms.txt at the landing commit, When
//! the served-pages test runs against the in-process server, Then every
//! same-host URL returns 200 with a non-empty body, including
//! `/status.html` and `/aup.html`. Replaces and generalises the
//! mcphost-polish-p0-20260930 hotfix's own narrower check (requirement 5).

use crate::common;
use common::TestServer;
use std::path::Path;

/// `mcphost.dev` is this host's own production hostname (`www/*.html`'s own
/// existing links already assume it, e.g. `status.html`'s "Endpoint:
/// `https://mcphost.dev/mcp`" line) -- an absolute link using it is exactly
/// as "same-host" as a bare `/path` one for this test's purpose.
const OWN_HOST_PREFIX: &str = "https://mcphost.dev";

/// Markdown's `[text](target)` link syntax, hand-rolled rather than adding
/// a `regex` dependency for one test -- every target this repo's docs use
/// is a single `](...)` span with no nested parens, which this handles.
fn extract_link_targets(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("](") {
        let after = &rest[start + 2..];
        let Some(end) = after.find(')') else { break };
        out.push(after[..end].to_string());
        rest = &after[end + 1..];
    }
    out
}

/// A target counts as "same-host" (requirement 5/AC6's own phrase) when
/// it's a bare absolute path on this binary, or an absolute link to this
/// host's own production hostname -- never a template placeholder
/// (`/t/{ns}/mcp`, `/help/<code>`) or an external site (GitHub, mailto,
/// discord, ...), neither of which this server itself serves at that path.
fn same_host_path(target: &str) -> Option<String> {
    let path = if let Some(p) = target.strip_prefix(OWN_HOST_PREFIX) {
        p
    } else if target.starts_with('/') && !target.starts_with("//") {
        target
    } else {
        return None;
    };
    let path = path.split('#').next().unwrap_or(path); // drop any #fragment
    if path.is_empty() || path == "/" {
        return None; // the bare host root isn't a page this test can GET meaningfully more than once
    }
    if path.contains('{') || path.contains('<') {
        return None; // a template placeholder, not a literal URL
    }
    Some(path.to_string())
}

fn collect_same_host_paths(text: &str, out: &mut std::collections::BTreeSet<String>) {
    for target in extract_link_targets(text) {
        if let Some(path) = same_host_path(&target) {
            out.insert(path);
        }
    }
}

fn find_markdown_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            find_markdown_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            out.push(path);
        }
    }
}

#[tokio::test]
async fn every_same_host_link_in_readme_docs_and_llms_txt_resolves() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut paths = std::collections::BTreeSet::new();

    let readme = std::fs::read_to_string(manifest_dir.join("README.md")).expect("README.md");
    collect_same_host_paths(&readme, &mut paths);

    let llms_txt =
        std::fs::read_to_string(manifest_dir.join("www/llms.txt")).expect("www/llms.txt");
    collect_same_host_paths(&llms_txt, &mut paths);

    let mut md_files = Vec::new();
    find_markdown_files(&manifest_dir.join("docs"), &mut md_files);
    assert!(!md_files.is_empty(), "docs/**/*.md must not be empty");
    for f in &md_files {
        let text = std::fs::read_to_string(f).unwrap_or_else(|e| panic!("read {f:?}: {e}"));
        collect_same_host_paths(&text, &mut paths);
    }

    // The two the hotfix found 404ing in prod must both be in scope, or
    // this test isn't actually generalising that check.
    assert!(paths.contains("/status.html"), "README/docs/llms.txt must still link /status.html: {paths:?}");
    assert!(paths.contains("/aup.html"), "README/docs/llms.txt must still link /aup.html: {paths:?}");
    assert!(paths.len() >= 4, "expected several distinct same-host links, got {paths:?}");

    let server = TestServer::start().await;
    let http = reqwest::Client::new();
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
        "every same-host link must resolve 200 with a non-empty body:\n{}",
        failures.join("\n")
    );
}
