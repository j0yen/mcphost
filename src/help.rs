//! `/help/<code>` -- the page a first-hour `help_url` resolves to, and the
//! small set of helpers every surface this PRD touches (`into_error_data`'s
//! `help_url`, `host.whoami`'s `links`, `www/support.html`, README/
//! `docs/agent-quickstart.md`/`www/llms.txt`'s support line, `docs/plans.md`)
//! shares so none of them can say something a different one disagrees with.
//!
//! PRD-mcphost-first-hour-support-surface requirement 2: "one paragraph per
//! code: meaning, likely cause, fix, link to the relevant doc section",
//! generated from one source. The literal source this crate's build has
//! available without a doc-comment-parsing proc-macro is this file's own
//! [`HELP_ENTRIES`] table, co-located with [`HELP_CODES`] right next to it --
//! `errors.rs`' AC2 test (every code in `HELP_CODES` has an entry here, and
//! every entry's code is a real `AppError::code()`) is what keeps the two
//! from drifting, the same guarantee a doc-comment extractor would buy at
//! much higher build-tooling cost for a 16-row table that changes rarely.

use serde_json::{Value, json};

/// Requirement 1 / AC1: every code a first-hour caller can receive that
/// gets a `help_url` and a `/help/<code>` page. Exactly the PRD's own list.
pub const HELP_CODES: &[&str] = &[
    "bearer_invalid",
    "tenant_key_missing",
    "tenant_key_invalid",
    "rate_limited",
    "signup_paused",
    "tool_not_found",
    "unknown_kind",
    "spec_too_large",
    "invalid_spec",
    "invalid_params",
    "host_not_allowed",
    "handle_taken",
    "handle_reserved",
    "spec_not_exposed",
    "service_unavailable",
    "internal",
];

/// One `/help/<code>` page's content: meaning, likely cause, fix, and which
/// doc section to read next -- requirement 2's own four-part shape.
pub struct HelpEntry {
    pub code: &'static str,
    pub meaning: &'static str,
    pub cause: &'static str,
    pub fix: &'static str,
    pub doc_link: &'static str,
}

/// The base this crate's generated docs link into on GitHub -- a public,
/// stable URL (not a secret, not environment-dependent), same host every
/// `www/*.html` page already links to its own source under
/// (`status.html`'s `docs/benchmarks/...` link is the precedent).
const REPO_DOCS_BASE: &str = "https://github.com/j0yen/mcphost/blob/main";

/// Requirement 2's table, one row per [`HELP_CODES`] entry, same order.
pub const HELP_ENTRIES: &[HelpEntry] = &[
    HelpEntry {
        code: "bearer_invalid",
        meaning: "The call carried no usable credential: no Authorization: Bearer header, and (for host.tool_publish and friends) no tenant_key argument either.",
        cause: "A fresh connection before signup, or a connection whose key never got attached after signup returned one.",
        fix: "Call signup first if you haven't; it returns a key. Then either send Authorization: Bearer <key> on the connection, or pass tenant_key as a call argument.",
        doc_link: "docs/agent-quickstart.md#quickstart-for-agents",
    },
    HelpEntry {
        code: "tenant_key_missing",
        meaning: "A tenant_key argument was required for this call (no Authorization header was sent on this connection) and none was given, or it wasn't a string.",
        cause: "Usually a client that reconnects without carrying the bearer header forward, falling back to the tenant_key argument form and forgetting to pass it.",
        fix: "Pass tenant_key: \"<the key signup returned>\" as a call argument, or attach Authorization: Bearer <key> to the connection instead.",
        doc_link: "docs/agent-quickstart.md#quickstart-for-agents",
    },
    HelpEntry {
        code: "tenant_key_invalid",
        meaning: "A tenant_key argument was present but matched no tenant.",
        cause: "A typo'd or truncated key, a key from a different host/environment, or a tenant that was deleted.",
        fix: "Call signup again for a new key, or double-check the value against what signup originally returned.",
        doc_link: "docs/agent-quickstart.md#quickstart-for-agents",
    },
    HelpEntry {
        code: "rate_limited",
        meaning: "signup's own per-source rate limit was exceeded.",
        cause: "More than the configured number of signups from the same source in the last hour -- usually a retry loop, not a real new user.",
        fix: "Wait and retry later (see retry_after_secs in the payload when present); if you're running a fleet of agents from one IP, ask the operator about the fleet-IP exemption.",
        doc_link: "www/support.html",
    },
    HelpEntry {
        code: "signup_paused",
        meaning: "The operator has paused new signups host-wide.",
        cause: "An abuse spike, a maintenance window, or a deliberate kill-switch flip.",
        fix: "Retry after retry_after_secs. An existing tenant's key keeps working; this only blocks new signups.",
        doc_link: "www/status.html",
    },
    HelpEntry {
        code: "tool_not_found",
        meaning: "The named tool doesn't exist for this tenant (or, for a shared/cross-tenant call, doesn't exist, isn't shared with you, or your group isn't in its sharing list).",
        cause: "A typo'd name, a tool that was removed, or a shared-tool call missing the owner namespace prefix.",
        fix: "Call host.tool_list() to see what you actually have; shared tools are called as <owner_namespace>.<tool> -- ask the owner to host.tool_share it with you or your group.",
        doc_link: "docs/agent-quickstart.md#quickstart-for-agents",
    },
    HelpEntry {
        code: "unknown_kind",
        meaning: "host.tool_publish's kind argument named a kind this host doesn't register.",
        cause: "A typo (\"pytho\", \"htpp\"), or a word from another platform's vocabulary (\"lambda\", \"function\") that doesn't map onto this host's kinds.",
        fix: "Check data.registered and data.did_you_mean in the error payload; two real kinds exist today, python and http (echo is a stub, not a real tool).",
        doc_link: "README.md#kinds",
    },
    HelpEntry {
        code: "spec_too_large",
        meaning: "The spec argument serializes to more bytes than this host allows for one tool.",
        cause: "An oversized source file, an embedded data blob, or a schema that grew past the 64 KiB spec cap.",
        fix: "Shrink the spec -- move large constant data into host.state or host.docs.put and read it at call time instead of inlining it in the spec.",
        doc_link: "README.md#call-limits",
    },
    HelpEntry {
        code: "invalid_spec",
        meaning: "The kind rejected the spec's shape -- a required field missing, or a field that doesn't parse the way that kind expects.",
        cause: "See data.field/data.expected in the payload: it names exactly which field and what was expected, in the same \"<field>: <expected>\" convention every kind uses.",
        fix: "Fix the named field (data.example in the payload, when present, is a working value for it) and republish, or dry-run first with host.tool_publish(..., dry_run: true).",
        doc_link: "docs/agent-quickstart.md#quickstart-for-agents",
    },
    HelpEntry {
        code: "invalid_params",
        meaning: "A control-plane call argument was missing, the wrong type, or otherwise malformed -- distinct from invalid_spec, which is about a tool's own spec.",
        cause: "A required argument omitted, or the wrong JSON type for one that was given.",
        fix: "Check data.field/data.expected when present, or re-read the tool's own schema from tools/list.",
        doc_link: "docs/agent-quickstart.md#quickstart-for-agents",
    },
    HelpEntry {
        code: "host_not_allowed",
        meaning: "An http-kind spec (or call) named a URL whose host this host's egress policy refuses to reach.",
        cause: "A private/internal address, or a public host your plan isn't allowed to reach (network: \"public\"/\"egress\" needs a paid plan).",
        fix: "Point the spec at a publicly routable host, or upgrade via billing.checkout if the policy is the actual blocker.",
        doc_link: "README.md#egress-proxy-network-public--egress",
    },
    HelpEntry {
        code: "handle_taken",
        meaning: "The agent-directory handle you tried to claim is already held by someone else.",
        cause: "Two agents picking the same obvious handle, or a handle you used before on a different tenant.",
        fix: "Pick a different handle.",
        doc_link: "docs/agent-quickstart.md#quickstart-for-agents",
    },
    HelpEntry {
        code: "handle_reserved",
        meaning: "The agent-directory handle you tried to claim is reserved by the operator (e.g. admin, host, mcphost, system).",
        cause: "A handle this host keeps for its own control-plane identity.",
        fix: "Pick a different handle; reserved handles are released only by an operator action, not by claiming around them.",
        doc_link: "docs/agent-quickstart.md#quickstart-for-agents",
    },
    HelpEntry {
        code: "spec_not_exposed",
        meaning: "A tool was shared with you, but not with expose_spec: true, so host.tool_spec_shared can't read its source.",
        cause: "The owner shared it for calling only (the default), not for forking.",
        fix: "You can still call it with host.tool_call; ask the owner to re-share with expose_spec: true if you need to read the spec.",
        doc_link: "docs/agent-quickstart.md#quickstart-for-agents",
    },
    HelpEntry {
        code: "service_unavailable",
        meaning: "The host refused the call because a resource it depends on (most commonly disk headroom) is below its safety floor.",
        cause: "The operator's own capacity, not anything about your call -- the payload deliberately carries no host numbers (request_id identifies the exact event in the server's own log for the operator).",
        fix: "Retry later; if it persists, report the request_id to the support channel below.",
        doc_link: "www/status.html",
    },
    HelpEntry {
        code: "internal",
        meaning: "Something failed on this host's side that isn't one of the named error codes above.",
        cause: "Could be almost anything server-side; the real detail is logged under this response's request_id and deliberately not sent to you.",
        fix: "Retry; if it keeps happening, report the request_id (from the payload) to the support channel below so an operator can look it up.",
        doc_link: "www/status.html",
    },
];

/// AC2's own test obligation, factored out so both `errors.rs`'s unit test
/// and any future caller can run it: every [`HELP_CODES`] entry has a row
/// here, and every row's code is one of [`HELP_CODES`] -- the two lists
/// can't drift apart silently.
pub fn entry(code: &str) -> Option<&'static HelpEntry> {
    HELP_ENTRIES.iter().find(|e| e.code == code)
}

/// `into_error_data`'s own `data.help_url` (requirement 1, AC1): `None` for
/// any code not in [`HELP_CODES`] (most `Structured` kind-error codes, which
/// have no generated page -- adding one is exactly "add it to `HELP_CODES`
/// and `HELP_ENTRIES`", no other call site changes).
pub fn help_url(base: &str, code: &str) -> Option<String> {
    if HELP_CODES.contains(&code) {
        Some(format!("{}/help/{code}", base.trim_end_matches('/')))
    } else {
        None
    }
}

/// `host.whoami`'s `links` (requirement 7, AC7): the four pointers an agent
/// (or its human) needs, all built from the same `base` every other
/// generated URL on this response already uses.
pub fn surface_links(base: &str) -> Value {
    let base = base.trim_end_matches('/');
    json!({
        "support": format!("{base}/support.html"),
        "plans": format!("{base}/plans.html"),
        "status": format!("{base}/status.html"),
        "help": format!("{base}/help"),
    })
}

/// Requirement 4 / AC5: the one line every surface (`www/support.html`,
/// README, `docs/agent-quickstart.md`, `www/llms.txt`, and every help
/// page's footer) renders `MCPHOST_SUPPORT_URL` into -- `Some` prints the
/// value verbatim (no interpretation: the operator picks Discord, GitHub
/// Discussions, or email and that's the whole contract), `None` prints the
/// literal phrase AC5 pins ("support channel not configured"), so a reader
/// (or a test's `.contains(...)`) never has to guess which state produced
/// a given page.
pub fn support_line(support_url: Option<&str>) -> String {
    match support_url {
        Some(url) if !url.trim().is_empty() => format!("Support: {}", url.trim()),
        _ => "Support: support channel not configured (MCPHOST_SUPPORT_URL is unset)."
            .to_string(),
    }
}

/// AC5's build-time surfaces: README, `docs/agent-quickstart.md`, and
/// `www/llms.txt` each carry one or more `<!-- support:start -->
/// ... <!-- support:end -->` sections (same marker convention
/// `llms_txt::TOOLS_SECTION_START`/`_END` already uses for the tools
/// section) that `gendocs::run` splices [`support_line`]'s current value
/// into at `mcphost gen-docs` time.
pub const SUPPORT_SECTION_START: &str = "<!-- support:start -->";
pub const SUPPORT_SECTION_END: &str = "<!-- support:end -->";

/// Replace every section bracketed by [`SUPPORT_SECTION_START`]/
/// [`SUPPORT_SECTION_END`] in `content` with [`support_line`]'s current
/// rendering of `support_url`, leaving everything outside the markers
/// (including surrounding blank lines) untouched -- unlike
/// `llms_txt::splice_into`, which also has to find a first-run insertion
/// point, every doc this is called on already carries the marker pair
/// committed, so there is no first-run case to handle. A doc with no
/// marker pair at all is returned unchanged rather than silently dropping
/// the section.
pub fn splice_support_section(content: &str, support_url: Option<&str>) -> String {
    let section = format!("{SUPPORT_SECTION_START}\n{}\n{SUPPORT_SECTION_END}", support_line(support_url));
    let mut out = String::new();
    let mut rest = content;
    loop {
        match (rest.find(SUPPORT_SECTION_START), rest.find(SUPPORT_SECTION_END)) {
            (Some(start), Some(end_marker_pos)) if end_marker_pos > start => {
                let end = end_marker_pos + SUPPORT_SECTION_END.len();
                out.push_str(&rest[..start]);
                out.push_str(&section);
                rest = &rest[end..];
            }
            _ => break,
        }
    }
    out.push_str(rest);
    out
}

/// requirement 4's other half, for the status feed (requirement 4: "the
/// readiness probe ... reports it as a warning -- never a silent blank"):
/// `Some(warning)` iff the support channel is unconfigured.
pub fn support_unconfigured_warning(support_url: Option<&str>) -> Option<&'static str> {
    match support_url {
        Some(url) if !url.trim().is_empty() => None,
        _ => Some("MCPHOST_SUPPORT_URL is unset; the support channel is not configured"),
    }
}

const PAGE_HEAD: &str = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">"##;

/// Shared with `status.html`/`aup.html`'s committed look (same palette,
/// same monospace stack) -- requirement 2/4's generated pages are new
/// files, not a new style.
const PAGE_STYLE: &str = r###"<style>
  :root { color-scheme: dark; }
  * { box-sizing: border-box; }
  body { margin: 0; background: #0b0d0e; color: #c9cdd1;
         font-family: ui-monospace, "Cascadia Mono", "DejaVu Sans Mono", Menlo, Consolas, monospace;
         font-size: 15px; line-height: 1.65; }
  .wrap { max-width: 44rem; margin: 0 auto; padding: 3.5rem 1.4rem 4rem; }
  h1 { font-size: 1.05rem; font-weight: 600; color: #e6edf3; margin: 0 0 .3rem; }
  h1::before { content: "# "; color: #484f58; font-weight: 400; }
  h2 { font-size: .95rem; font-weight: 600; color: #e6edf3; margin: 2.2rem 0 .5rem; }
  h2::before { content: "## "; color: #484f58; font-weight: 400; }
  p, ul { margin: 0 0 1rem; }
  code { color: #e6edf3; background: #161b22; padding: .1em .35em; border-radius: 4px; }
  a { color: #58a6ff; text-decoration: none; }
  a:hover { text-decoration: underline; }
  ul { list-style: none; padding-left: 1.3rem; }
  li::before { content: "- "; color: #484f58; margin-left: -1.3rem; }
  strong { color: #e6edf3; font-weight: 600; }
  footer { margin-top: 3rem; padding-top: 1.2rem; border-top: 1px solid #21262d;
           color: #8b949e; font-size: .9em; }
</style>"###;

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Requirement 2 / AC1-AC2: one `/help/<code>` page -- meaning, likely
/// cause, fix, and the doc section to read next, plus the same support
/// footer every other surface carries.
pub fn render_help_page(e: &HelpEntry, support_url: Option<&str>) -> String {
    format!(
        r##"{head}
<title>mcphost — help: {code}</title>
{style}
</head>
<body>
<main class="wrap">
<h1>help: {code}</h1>
<p><strong>What it means:</strong> {meaning}</p>
<p><strong>Likely cause:</strong> {cause}</p>
<p><strong>Fix:</strong> {fix}</p>
<p>See also: <a href="{doc_href}">{doc_link}</a></p>
<footer>{support} · <a href="/help">all codes</a> · <a href="/status.html">status</a></footer>
</main>
</body>
</html>
"##,
        head = PAGE_HEAD,
        style = PAGE_STYLE,
        code = html_escape(e.code),
        meaning = html_escape(e.meaning),
        cause = html_escape(e.cause),
        fix = html_escape(e.fix),
        doc_href = doc_href(e.doc_link),
        doc_link = html_escape(e.doc_link),
        support = html_escape(&support_line(support_url)),
    )
}

/// A `doc_link` that already names a `www/*.html` page is served by this
/// same binary at that path; everything else (README/docs anchors) is read
/// on GitHub, same convention `status.html`'s own benchmark link uses.
fn doc_href(doc_link: &str) -> String {
    if let Some(rest) = doc_link.strip_prefix("www/") {
        format!("/{rest}")
    } else {
        format!("{REPO_DOCS_BASE}/{doc_link}")
    }
}

/// `GET /help` (requirement 7's `links.help`): an index of every generated
/// code page, so that link itself resolves to something a human can read
/// rather than a 404 (`/help/<code>` alone has no index otherwise).
pub fn render_help_index(support_url: Option<&str>) -> String {
    let items: String = HELP_ENTRIES
        .iter()
        .map(|e| format!(r##"<li><a href="/help/{code}">{code}</a> -- {meaning}</li>"##, code = html_escape(e.code), meaning = html_escape(e.meaning)))
        .collect();
    format!(
        r##"{head}
<title>mcphost — help</title>
{style}
</head>
<body>
<main class="wrap">
<h1>help</h1>
<p>Every error code a first-hour call can return, each with its own meaning/cause/fix.</p>
<ul>{items}</ul>
<footer>{support} · <a href="/status.html">status</a></footer>
</main>
</body>
</html>
"##,
        head = PAGE_HEAD,
        style = PAGE_STYLE,
        items = items,
        support = html_escape(&support_line(support_url)),
    )
}

/// `GET /support.html` (requirement 4): the one page every other surface's
/// support line points at.
pub fn render_support_page(support_url: Option<&str>) -> String {
    let body = match support_url {
        Some(url) if !url.trim().is_empty() => format!(
            r##"<p>Need a human? <a href="{url}">{url}</a></p>"##,
            url = html_escape(url.trim())
        ),
        _ => "<p>support channel not configured (MCPHOST_SUPPORT_URL is unset) -- \
              file an issue instead: <a href=\"https://github.com/j0yen/mcphost/issues\">\
              github.com/j0yen/mcphost/issues</a></p>"
            .to_string(),
    };
    format!(
        r##"{head}
<title>mcphost — support</title>
{style}
</head>
<body>
<main class="wrap">
<h1>support</h1>
{body}
<p>Something a specific error said to come here about? Every error payload carries a <code>request_id</code> -- include it.</p>
<footer><a href="/">mcphost</a> · <a href="/status.html">status</a> · <a href="/plans.html">plans</a> · <a href="/help">help</a></footer>
</main>
</body>
</html>
"##,
        head = PAGE_HEAD,
        style = PAGE_STYLE,
        body = body,
    )
}

// ---- requirement 8: a view counter for `/help/<code>` ----------------------

/// A process-wide, in-memory hit counter keyed by help code (requirement 8:
/// "`docs/metrics.md` names a counter `help_url_served{code}` so Joe can see
/// which errors developers actually hit"). Deliberately not a per-tenant or
/// per-request-scoped thing (unlike `help_url` itself) -- it's host-wide
/// telemetry, the same shape as `hooks::EventCounters`, just keyed by code
/// instead of a single received/rejected pair. A global rather than an
/// `AppState` field on purpose: every hermetic test builds its own
/// short-lived `AppState`, and this counter's whole point is to be visible
/// on `/healthz` across the process's real lifetime, not reset per test
/// server -- the same reason it's fine for two tests running concurrently
/// in one `cargo test` binary to share one count.
static HELP_HITS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<&'static str, u64>>> =
    std::sync::OnceLock::new();

fn help_hits() -> &'static std::sync::Mutex<std::collections::HashMap<&'static str, u64>> {
    HELP_HITS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Every `GET /help/<code>` that resolves to a real page records one hit.
pub fn record_help_served(code: &'static str) {
    if let Ok(mut guard) = help_hits().lock() {
        *guard.entry(code).or_insert(0) += 1;
    }
}

/// `/healthz`'s `help_url_served` object -- `{code: count}` for every code
/// served at least once since this process started.
pub fn help_hits_snapshot() -> Value {
    let guard = help_hits().lock().unwrap_or_else(|e| e.into_inner());
    json!(
        guard
            .iter()
            .map(|(k, v)| (k.to_string(), *v))
            .collect::<std::collections::HashMap<String, u64>>()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AC2: every code in `HELP_CODES` has a page here.
    #[test]
    fn every_help_code_has_an_entry() {
        for code in HELP_CODES {
            assert!(
                entry(code).is_some(),
                "HELP_CODES names {code} but HELP_ENTRIES has no row for it"
            );
        }
    }

    /// AC2's other half: every page's code exists (is one of HELP_CODES --
    /// the pairing errors.rs's own test extends to "and is a real
    /// AppError::code()").
    #[test]
    fn every_entry_code_is_in_help_codes() {
        for e in HELP_ENTRIES {
            assert!(
                HELP_CODES.contains(&e.code),
                "HELP_ENTRIES has a row for {} not listed in HELP_CODES",
                e.code
            );
        }
    }

    #[test]
    fn help_url_is_none_for_an_unlisted_code() {
        assert_eq!(help_url("https://mcphost.dev", "quota_exceeded"), None);
    }

    #[test]
    fn help_url_strips_trailing_slash_on_base() {
        assert_eq!(
            help_url("https://mcphost.dev/", "bearer_invalid").as_deref(),
            Some("https://mcphost.dev/help/bearer_invalid")
        );
    }

    #[test]
    fn support_line_contains_url_when_set() {
        assert!(support_line(Some("https://discord.gg/example")).contains("https://discord.gg/example"));
    }

    #[test]
    fn support_line_names_not_configured_when_unset() {
        assert!(support_line(None).contains("support channel not configured"));
        assert!(support_line(Some("")).contains("support channel not configured"));
    }

    #[test]
    fn help_hits_counter_increments() {
        let before = help_hits_snapshot()["bearer_invalid"].as_u64().unwrap_or(0);
        record_help_served("bearer_invalid");
        let after = help_hits_snapshot()["bearer_invalid"].as_u64().unwrap_or(0);
        assert_eq!(after, before + 1);
    }

    #[test]
    fn splice_support_section_replaces_one_section_leaving_the_rest_untouched() {
        let content = format!(
            "before\n\n{SUPPORT_SECTION_START}\nSupport: old\n{SUPPORT_SECTION_END}\n\nafter\n"
        );
        let spliced = splice_support_section(&content, Some("https://discord.gg/example"));
        assert!(spliced.contains("https://discord.gg/example"));
        assert!(!spliced.contains("Support: old"));
        assert!(spliced.starts_with("before\n\n"));
        assert!(spliced.ends_with("\n\nafter\n"));
    }

    #[test]
    fn splice_support_section_replaces_every_occurrence() {
        let content = format!(
            "one\n\n{SUPPORT_SECTION_START}\nSupport: old\n{SUPPORT_SECTION_END}\n\ntwo\n\n\
             {SUPPORT_SECTION_START}\nSupport: old\n{SUPPORT_SECTION_END}\n\nthree\n"
        );
        let spliced = splice_support_section(&content, Some("https://discord.gg/example"));
        assert_eq!(spliced.matches("https://discord.gg/example").count(), 2);
        assert!(!spliced.contains("Support: old"));
    }

    #[test]
    fn splice_support_section_with_no_url_is_idempotent_on_already_unset_content() {
        let content = format!(
            "before\n\n{SUPPORT_SECTION_START}\n{}\n{SUPPORT_SECTION_END}\n\nafter\n",
            support_line(None)
        );
        assert_eq!(splice_support_section(&content, None), content);
    }

    #[test]
    fn splice_support_section_leaves_content_with_no_markers_unchanged() {
        let content = "no markers here\n";
        assert_eq!(splice_support_section(content, Some("https://discord.gg/example")), content);
    }
}
