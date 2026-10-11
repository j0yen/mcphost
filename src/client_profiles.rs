//! PRD-mcphost-client-profile-registry: one table of the constraints each of
//! the 13 install surfaces documents, keyed by [`SURFACE_IDS`]. Every row
//! carries the `doc_url` its figures come from and the date it was checked.
//! `clientinfo_prefixes` is collected from [`CLIENT_INFO_NAMES`] by the
//! `prefixes!` macro below, never typed a second time.

use serde::Serialize;

use crate::install_links::CLIENT_INFO_NAMES;
use crate::state::MAX_TOOL_OUTPUT_BYTES;

/// Latest `checked` date across [`CLIENT_PROFILES`].
pub const CHECKED: &str = "2026-10-09";

/// Whether a client needs OAuth to reach a remote MCP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Oauth {
    None,
    Optional,
    Required,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClientProfile {
    pub id: &'static str,
    pub clientinfo_prefixes: &'static [&'static str],
    pub user_agent_prefixes: &'static [&'static str],
    pub tool_name_max: u16,
    pub nested_depth_max: u8,
    pub structured_content: bool,
    pub oauth: Oauth,
    /// Bytes of `content[0].text` the client renders before cutting off.
    pub result_bytes_max: u32,
    pub doc_url: &'static str,
    pub checked: &'static str,
}

const fn bytes_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// How many [`CLIENT_INFO_NAMES`] entries map to `id`.
const fn prefix_count(id: &str) -> usize {
    let mut n = 0;
    let mut i = 0;
    while i < CLIENT_INFO_NAMES.len() {
        if bytes_eq(CLIENT_INFO_NAMES[i].1, id) {
            n += 1;
        }
        i += 1;
    }
    n
}

/// The `N` [`CLIENT_INFO_NAMES`] prefixes that map to `id`, in table order.
const fn collect_prefixes<const N: usize>(id: &str) -> [&'static str; N] {
    let mut out = [""; N];
    let mut n = 0;
    let mut i = 0;
    while i < CLIENT_INFO_NAMES.len() {
        if bytes_eq(CLIENT_INFO_NAMES[i].1, id) {
            out[n] = CLIENT_INFO_NAMES[i].0;
            n += 1;
        }
        i += 1;
    }
    out
}

macro_rules! prefixes {
    ($id:literal) => {{
        const ARR: [&str; prefix_count($id)] = collect_prefixes::<{ prefix_count($id) }>($id);
        &ARR
    }};
}

macro_rules! row {
    ($id:literal, $ua:expr, $name:literal, $depth:literal, $sc:literal, $oauth:ident, $bytes:literal, $url:literal) => {
        ClientProfile {
            id: $id,
            clientinfo_prefixes: prefixes!($id),
            user_agent_prefixes: $ua,
            tool_name_max: $name,
            nested_depth_max: $depth,
            structured_content: $sc,
            oauth: Oauth::$oauth,
            result_bytes_max: $bytes,
            doc_url: $url,
            checked: CHECKED,
        }
    };
}

/// One row per [`crate::install_links::SURFACE_IDS`] entry, in the same
/// order (asserted by `tests/cprf_ac02_*`).
pub static CLIENT_PROFILES: [ClientProfile; 13] = [
    row!("claude_code", &["claude-code"], 64, 8, true, None, 1_000_000, "https://code.claude.com/docs/en/mcp"),
    row!("cursor", &["cursor"], 60, 3, false, Optional, 100_000, "https://docs.cursor.com/en/tools/mcp"),
    row!("vscode", &["vscode"], 64, 8, true, Optional, 1_000_000, "https://code.visualstudio.com/api/extension-guides/ai/mcp"),
    row!("claude_ai", &[], 64, 4, true, Required, 1_000_000, "https://support.claude.com/en/articles/11175166-get-started-with-custom-connectors-using-remote-mcp"),
    row!("chatgpt", &[], 64, 2, true, Required, 100_000, "https://platform.openai.com/docs/mcp"),
    row!("codex_cli", &["codex"], 64, 8, false, None, 1_000_000, "https://developers.openai.com/codex/mcp"),
    row!("gemini_cli", &["gemini-cli"], 63, 4, false, None, 1_000_000, "https://geminicli.com/docs/tools/mcp-server/"),
    row!("opencode", &[], 64, 8, false, None, 1_000_000, "https://opencode.ai/docs/mcp-servers/"),
    row!("amp", &[], 64, 8, false, None, 1_000_000, "https://ampcode.com/manual#mcp"),
    row!("goose", &[], 64, 8, false, None, 1_000_000, "https://goose-docs.ai/docs/getting-started/using-extensions/"),
    row!("warp", &[], 64, 8, false, None, 1_000_000, "https://docs.warp.dev/agents/capabilities/mcp/"),
    row!("windsurf", &[], 64, 8, false, None, 1_000_000, "https://docs.windsurf.com/windsurf/cascade/mcp"),
    row!("cline", &[], 64, 8, false, None, 1_000_000, "https://docs.cline.bot/mcp/configuring-mcp-servers"),
];

/// What the host assumes when no signal names a client: the host maxima, so
/// nothing is adapted away from a caller we cannot identify.
pub static UNKNOWN: ClientProfile = ClientProfile {
    id: "unknown",
    clientinfo_prefixes: &[],
    user_agent_prefixes: &[],
    tool_name_max: 41,
    nested_depth_max: 8,
    structured_content: true,
    oauth: Oauth::None,
    result_bytes_max: MAX_TOOL_OUTPUT_BYTES as u32,
    doc_url: "",
    checked: "",
};

/// The `doc_url` of every non-unknown row, in table order -- what
/// `scripts/www-check.sh --client-docs` fetches.
pub fn doc_urls() -> Vec<&'static str> {
    CLIENT_PROFILES.iter().map(|p| p.doc_url).collect()
}

/// Which request signal named the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum From {
    #[serde(rename = "clientInfo")]
    ClientInfo,
    #[serde(rename = "userAgent")]
    UserAgent,
}

impl From {
    pub fn as_str(self) -> &'static str {
        match self {
            From::ClientInfo => "clientInfo",
            From::UserAgent => "userAgent",
        }
    }
}

fn first_match(
    haystack: &str,
    prefixes: impl Fn(&'static ClientProfile) -> &'static [&'static str],
) -> Option<&'static ClientProfile> {
    let haystack = haystack.to_ascii_lowercase();
    CLIENT_PROFILES
        .iter()
        .find(|p| prefixes(p).iter().any(|prefix| haystack.starts_with(prefix)))
}

/// Resolve a request to one profile. `client_info` is the `(name, version)`
/// the client announced in `initialize`; lower-cased prefix match on
/// `clientinfo_prefixes` wins, then `user_agent_prefixes`, else [`UNKNOWN`]
/// with no `From` (the host guesses nothing).
pub fn profile_for(
    client_info: Option<(&str, &str)>,
    user_agent: Option<&str>,
) -> (&'static ClientProfile, Option<From>) {
    if let Some((name, _version)) = client_info
        && let Some(p) = first_match(name, |p| p.clientinfo_prefixes)
    {
        return (p, Some(From::ClientInfo));
    }
    if let Some(ua) = user_agent
        && let Some(p) = first_match(ua, |p| p.user_agent_prefixes)
    {
        return (p, Some(From::UserAgent));
    }
    (&UNKNOWN, None)
}

/// The `client_profile` object `host.whoami` reports: the resolved row's
/// seven constraint fields plus which signal named it (`null` for `unknown`).
pub fn whoami_json(profile: &ClientProfile, from: Option<From>) -> serde_json::Value {
    serde_json::json!({
        "id": profile.id,
        "from": from.map(From::as_str),
        "tool_name_max": profile.tool_name_max,
        "nested_depth_max": profile.nested_depth_max,
        "structured_content": profile.structured_content,
        "oauth": profile.oauth,
        "result_bytes_max": profile.result_bytes_max,
    })
}

/// Route of the public JSON rendering of [`CLIENT_PROFILES`].
pub const JSON_ROUTE: &str = "/client-profiles.json";
/// Committed copies written by `mcphost gen-docs`.
pub const JSON_PATH: &str = "www/client-profiles.json";
pub const DOC_PATH: &str = "docs/client-profiles.md";

/// The public JSON: `{generated_from, checked, profiles}`, pretty-printed with
/// a trailing newline. `generated_from` is a parameter so the served route
/// can report the running version while the committed copy is compared by
/// re-rendering with the version it records (a release bumps `Cargo.toml`
/// after this file is committed, as `registry_manifest` already documents).
pub fn render_json(generated_from: &str) -> String {
    let doc = serde_json::json!({
        "generated_from": generated_from,
        "checked": CHECKED,
        "profiles": CLIENT_PROFILES,
    });
    let mut out = serde_json::to_string_pretty(&doc).unwrap_or_default();
    out.push('\n');
    out
}

/// `docs/client-profiles.md`: every row's figures beside the URL they cite.
pub fn render_markdown() -> String {
    let mut out = String::new();
    out.push_str("# Client profiles\n\n");
    out.push_str(
        "What each of the 13 MCP install surfaces can carry, one row per `SURFACE_IDS` entry. \
         Generated by `mcphost gen-docs` from `CLIENT_PROFILES` in `src/client_profiles.rs`; \
         do not hand-edit. The same rows are served at `/client-profiles.json`, and \
         `host.whoami` reports the row a session resolved to.\n\n",
    );
    out.push_str(
        "The figures are drafted defaults, to be replaced by the documented figure wherever the \
         linked page states one; `checked` is the date the Docs link last resolved.\n\n",
    );
    out.push_str(
        "| id | tool name max | nested depth max | structuredContent | OAuth | result bytes max | Docs | checked |\n\
         |---|---|---|---|---|---|---|---|\n",
    );
    for p in &CLIENT_PROFILES {
        out.push_str(&format!(
            "| `{}` | {} | {} | {} | {} | {} | <{}> | {} |\n",
            p.id,
            p.tool_name_max,
            p.nested_depth_max,
            if p.structured_content { "yes" } else { "no" },
            match p.oauth {
                Oauth::None => "none",
                Oauth::Optional => "optional",
                Oauth::Required => "required",
            },
            p.result_bytes_max,
            p.doc_url,
            p.checked,
        ));
    }
    out.push_str(&format!(
        "\nA caller no signal identifies resolves to `unknown`: {} / {} / {} / none / {}.\n",
        UNKNOWN.tool_name_max,
        UNKNOWN.nested_depth_max,
        if UNKNOWN.structured_content { "yes" } else { "no" },
        UNKNOWN.result_bytes_max,
    ));
    out
}

/// A row whose `checked` date is more than this many days old is stale.
pub const MAX_AGE_DAYS: i64 = 90;

/// Days since 1970-01-01 for a `YYYY-MM-DD` date (proleptic Gregorian).
pub fn days_from_date(date: &str) -> Option<i64> {
    let mut parts = date.split('-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: i64 = parts.next()?.parse().ok()?;
    let d: i64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

/// Today's UTC date as `YYYY-MM-DD`.
pub fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let z = secs.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// One message per row whose `checked` date is more than [`MAX_AGE_DAYS`]
/// before `today` (or does not parse), each naming the row and its `doc_url`.
pub fn stale_rows(today: &str) -> Vec<String> {
    let Some(now) = days_from_date(today) else {
        return vec![format!("today {today:?} is not a YYYY-MM-DD date")];
    };
    CLIENT_PROFILES
        .iter()
        .filter_map(|p| match days_from_date(p.checked) {
            Some(then) if now - then <= MAX_AGE_DAYS => None,
            Some(then) => Some(format!(
                "client profile `{}` was last checked {} ({} days ago, limit {MAX_AGE_DAYS}); re-verify {}",
                p.id,
                p.checked,
                now - then,
                p.doc_url
            )),
            None => Some(format!("client profile `{}` has an unparseable checked date {:?}; re-verify {}", p.id, p.checked, p.doc_url)),
        })
        .collect()
}
