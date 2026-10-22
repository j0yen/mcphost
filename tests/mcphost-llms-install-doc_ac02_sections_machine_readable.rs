//! AC2 (hotfix install-doc-copilot-sdk) -- every install surface in
//! `SURFACE_IDS` has a `## ` section the discovery chain's install-walk can
//! parse: a heading ending in (`<id>`), then a first fenced block holding a
//! deeplink, a JSON object with a server url, or an `mcp add` command with
//! an endpoint. Two surfaces users install from (`copilot_vscode`, `sdk`)
//! had no section, so the walk logged `missing_section`.
//!
//! The grammar below is a pinned copy of the walker's (synthorg
//! `install_walk.py`): the heading regex at :79
//! (`^## .*\(`([A-Za-z0-9_\-]+)`\)\s*$`) and the first-parseable-block rules
//! at :769-777 (a `cursor://` or `vscode:mcp/install?` deeplink; a
//! ```json/```jsonc fence whose object carries a `url` or `serverUrl` string
//! at top level, under `mcpServers`/`servers`, or as a value; or a
//! `... mcp add` line with `--url` or a bare http(s) token). If the walker's
//! grammar changes, change this copy with it.
//!
//! `Kind::Steps` surfaces (Claude.ai, ChatGPT, Goose, Warp) are numbered
//! instructions with no machine-readable install path by design; they need
//! the heading but are exempt from the block rule.

use mcphost::install_links::{self, CANONICAL_PUBLIC_URL, Kind, SURFACE_IDS};

/// The walker's heading regex, by hand (the crate has no regex dependency):
/// `## ` then anything, then (`id`) at end of line (trailing whitespace ok),
/// `id` being `[A-Za-z0-9_-]+`.
fn heading_id(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("## ")?.trim_end();
    let inner = rest.strip_suffix("`)")?;
    let id = &inner[inner.rfind("(`")? + 2..];
    (!id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')).then_some(id)
}

/// `(language, body)` fenced blocks of one section body, in order, plus the
/// deeplink-bearing plain lines are not needed: every deeplink in this doc
/// sits inside a fence.
fn fences(body: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut open: Option<(String, String)> = None;
    for line in body.lines() {
        if let Some(lang) = line.strip_prefix("```") {
            match open.take() {
                Some(done) => out.push(done),
                None => open = Some((lang.trim().to_string(), String::new())),
            }
        } else if let Some((_, text)) = open.as_mut() {
            text.push_str(line);
            text.push('\n');
        }
    }
    out
}

fn has_url(v: &serde_json::Value) -> bool {
    let direct = |o: &serde_json::Value| {
        ["url", "serverUrl"].iter().any(|k| o.get(*k).is_some_and(|u| u.is_string()))
    };
    if direct(v) {
        return true;
    }
    ["mcpServers", "servers"]
        .iter()
        .filter_map(|k| v.get(*k).and_then(|m| m.as_object()))
        .any(|m| m.values().any(direct))
}

/// Why `(lang, text)` is not a parseable install block, or `None` when it is.
fn parse_problem(lang: &str, text: &str) -> Option<String> {
    if text.lines().any(|l| l.contains("cursor://") || l.contains("vscode:mcp/install?")) {
        return None;
    }
    if matches!(lang, "json" | "jsonc") {
        return match serde_json::from_str::<serde_json::Value>(text) {
            Ok(v) if has_url(&v) => None,
            Ok(_) => Some("json fence has no url/serverUrl".to_string()),
            Err(e) => Some(format!("json fence does not parse: {e}")),
        };
    }
    let add = text.lines().find(|l| l.contains("mcp add"));
    match add {
        Some(l)
            if ["--url", "--endpoint", "--server-url"].iter().any(|f| l.contains(f))
                || l.split_whitespace().any(|t| t.starts_with("http://") || t.starts_with("https://")) =>
        {
            None
        }
        Some(l) => Some(format!("`mcp add` line has no endpoint: {l}")),
        None => Some(format!("fence ({lang}) is neither deeplink, json nor `mcp add`")),
    }
}

#[test]
fn every_surface_id_has_a_walker_parseable_section() {
    let doc = install_links::render_install_doc(CANONICAL_PUBLIC_URL);
    let links = install_links::for_url(CANONICAL_PUBLIC_URL);

    // The two surfaces the install-walk reported as `missing_section`.
    for id in ["copilot_vscode", "sdk"] {
        assert!(SURFACE_IDS.contains(&id), "{id} must be an install surface");
    }

    let mut problems: Vec<String> = Vec::new();
    for id in SURFACE_IDS {
        // The heading, then the section body up to the next `## `.
        let mut lines = doc.lines().skip_while(|l| heading_id(l) != Some(id));
        if lines.next().is_none() {
            problems.push(format!("{id}: no heading matching the walker regex"));
            continue;
        }
        let body: String = lines.take_while(|l| !l.starts_with("## ")).map(|l| format!("{l}\n")).collect();
        let kind = links.iter().find(|s| s.id == id).map(|s| s.kind);
        if kind == Some(Kind::Steps) {
            continue;
        }
        match fences(&body).first() {
            None => problems.push(format!("{id}: no fenced block after the heading")),
            Some((lang, text)) => {
                if let Some(why) = parse_problem(lang, text) {
                    problems.push(format!("{id}: {why}"));
                }
            }
        }
    }
    assert!(problems.is_empty(), "install doc sections the walker cannot parse:\n{}", problems.join("\n"));
}

#[test]
fn each_section_heading_matches_exactly_once() {
    let doc = install_links::render_install_doc(CANONICAL_PUBLIC_URL);
    for id in SURFACE_IDS {
        let n = doc.lines().filter(|l| heading_id(l) == Some(id)).count();
        assert_eq!(n, 1, "{id} must have exactly one walker-matching heading");
    }
}

#[test]
fn the_new_sections_carry_the_endpoint_from_public_url() {
    let doc = install_links::render_install_doc("https://example.test");
    for (id, needle) in [
        ("copilot_vscode", "\"servers\""),
        ("sdk", "MCPServerStreamableHttp(params={\"url\": \"https://example.test/mcp\"})"),
    ] {
        let body: String = doc
            .lines()
            .skip_while(|l| heading_id(l) != Some(id))
            .skip(1)
            .take_while(|l| !l.starts_with("## "))
            .map(|l| format!("{l}\n"))
            .collect();
        assert!(!body.contains("mcphost.dev"), "{id}: no literal production host: {body}");
        assert!(body.contains("https://example.test/mcp"), "{id}: {body}");
        assert!(body.contains(needle), "{id} must contain {needle}: {body}");
    }
}
