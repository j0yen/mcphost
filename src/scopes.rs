//! PRD-mcphost-key-delegate-child-keys R3: the scopes a delegated key
//! carries and the one rule that says which scope a verb needs.
//!
//! `mcp` is the primary key's scope and implies every other one, so a key
//! holding only `["mcp"]` (the primary, and every `host.key.mint` sub-key)
//! behaves exactly as it did before scopes existed.

use crate::subkeys::ALWAYS_LISTED;

/// One authority a credential can hold. The single registry every scope
/// word in mcphost comes from (PRD-mcphost-scope-registry-generated).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scope {
    /// Everything; the primary key's scope.
    Mcp,
    /// OAuth built-in: read access.
    Read,
    /// OAuth built-in: write access.
    Write,
    /// List and inspect tools.
    ToolsRead,
    /// Call the tenant's published tools (implies `tools:read`).
    ToolsCall,
    /// Call the `host.*` / `billing.*` management verbs (implies `tools:call`).
    ToolsWrite,
    /// Read tenant state.
    StateRead,
    /// Write tenant state (implies `state:read`).
    StateWrite,
    /// Write secrets.
    SecretsWrite,
    /// Mint, rotate and revoke keys.
    KeysManage,
    /// Tenant administration.
    Admin,
}

/// The refusal code for a credential lacking a required scope.
pub const INSUFFICIENT_SCOPE: &str = "insufficient_scope";
/// The refusal code for a delegation request that exceeds its parent.
pub const SCOPE_ESCALATION: &str = "scope_escalation";
/// The refusal code for a scope word in no registry or catalog.
pub const SCOPE_UNKNOWN: &str = "scope_unknown";

impl Scope {
    /// Every scope, in registry order.
    pub const ALL: &'static [Scope] = &[
        Scope::Mcp,
        Scope::Read,
        Scope::Write,
        Scope::ToolsRead,
        Scope::ToolsCall,
        Scope::ToolsWrite,
        Scope::StateRead,
        Scope::StateWrite,
        Scope::SecretsWrite,
        Scope::KeysManage,
        Scope::Admin,
    ];

    /// The words the OAuth metadata document advertises as
    /// `scopes_supported` once a tenant has a catalog: the pre-registry
    /// `mcp`/`read`/`write` trio (the rest are accepted but not advertised).
    pub const ADVERTISED: &'static [Scope] = &[Scope::Mcp, Scope::Read, Scope::Write];

    pub const UNKNOWN_CODE: &'static str = SCOPE_UNKNOWN;

    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Mcp => "mcp",
            Scope::Read => "read",
            Scope::Write => "write",
            Scope::ToolsRead => "tools:read",
            Scope::ToolsCall => "tools:call",
            Scope::ToolsWrite => "tools:write",
            Scope::StateRead => "state:read",
            Scope::StateWrite => "state:write",
            Scope::SecretsWrite => "secrets:write",
            Scope::KeysManage => "keys:manage",
            Scope::Admin => "admin",
        }
    }

    /// The default consent-page / `host.oauth.scopes` description.
    pub fn description(self) -> &'static str {
        match self {
            Scope::Mcp => "Full access to this tenant's tools and verbs.",
            Scope::Read => "Read access to the tools that declare it.",
            Scope::Write => "Write access to the tools that declare it.",
            Scope::ToolsRead => "List and inspect tools.",
            Scope::ToolsCall => "Call published tools (includes tools:read).",
            Scope::ToolsWrite => "Publish, change and remove tools and call management verbs (includes tools:call).",
            Scope::StateRead => "Read tenant state.",
            Scope::StateWrite => "Write tenant state (includes state:read).",
            Scope::SecretsWrite => "Set and delete secrets.",
            Scope::KeysManage => "Mint, rotate and revoke keys.",
            Scope::Admin => "Tenant administration.",
        }
    }

    pub fn parse(s: &str) -> Option<Scope> {
        Scope::ALL.iter().copied().find(|sc| sc.as_str() == s)
    }

    /// Whether holding `self` grants `other` (reflexive).
    pub fn implies(self, other: Scope) -> bool {
        if self == other || self == Scope::Mcp {
            return true;
        }
        matches!(
            (self, other),
            (Scope::ToolsWrite, Scope::ToolsCall | Scope::ToolsRead)
                | (Scope::ToolsCall, Scope::ToolsRead)
                | (Scope::StateWrite, Scope::StateRead)
        )
    }

    /// The scopes `self` grants beyond itself, in registry order.
    pub fn implied(self) -> Vec<Scope> {
        Scope::ALL.iter().copied().filter(|o| *o != self && self.implies(*o)).collect()
    }
}

/// Every registry word, in registry order.
pub fn all_names() -> Vec<&'static str> {
    Scope::ALL.iter().map(|s| s.as_str()).collect()
}

tokio::task_local! {
    /// The calling OAuth token's `scope` claim (`None` for a key or a
    /// claimless token), set by `handler::call_tool_inner` around dispatch so
    /// `host.whoami` can report `scopes.effective` without a parameter
    /// threaded through three dispatch layers.
    pub static CALL_SCOPE: Option<String>;
}

/// The words the current caller effectively holds: `["mcp"]` for a key or a
/// claimless token, else the token's own words that are registry members or
/// in `catalog` (an unknown word was dropped at mint and is dropped here too).
pub fn effective_for(token_scope: Option<&str>, catalog: &[String]) -> Vec<String> {
    let Some(scope) = token_scope else {
        return vec![Scope::Mcp.as_str().to_string()];
    };
    let mut out: Vec<String> = Vec::new();
    for w in scope.split_whitespace() {
        let known = Scope::parse(w).is_some() || catalog.iter().any(|c| c == w);
        if known && !out.iter().any(|o| o == w) {
            out.push(w.to_string());
        }
    }
    out
}

/// The `scopes` object of `host.whoami`.
pub fn whoami_json(token_scope: Option<&str>, catalog: &[String]) -> serde_json::Value {
    serde_json::json!({"all": all_names(), "effective": effective_for(token_scope, catalog)})
}

/// The one scope gate shared by `tools/list` and `tools/call`.
///
/// `token_scope` is an OAuth token's space-separated `scope` claim (`None`
/// for a key or a claimless token: unrestricted). `required` is the tool's
/// own declared scopes (empty means `mcp`). A held word satisfies a required
/// word when it is that word or, for registry words, [`Scope::implies`] it;
/// a word outside the registry (a tenant catalog scope) matches only itself.
pub fn check(token_scope: Option<&str>, required: &[String]) -> bool {
    let Some(scope) = token_scope else {
        return true;
    };
    let held: Vec<&str> = scope.split_whitespace().collect();
    if held.contains(&Scope::Mcp.as_str()) {
        return true;
    }
    let mcp = [Scope::Mcp.as_str().to_string()];
    let required = if required.is_empty() { &mcp[..] } else { required };
    required.iter().all(|r| {
        held.iter().any(|h| {
            *h == r.as_str()
                || matches!((Scope::parse(h), Scope::parse(r)), (Some(h), Some(r)) if h.implies(r))
        })
    })
}

/// Whether any scope in `held` grants `want`.
pub fn held_implies(held: &[Scope], want: Scope) -> bool {
    held.iter().any(|h| h.implies(want))
}

/// The scope a call to the wire name `name` needs: `None` for the identity
/// verbs every key may call, `tools:call` for a published tool, and
/// `tools:write` for every other `host.*` / `billing.*` verb.
pub fn required_for(name: &str) -> Option<Scope> {
    if ALWAYS_LISTED.contains(&name) || name == "host.key.delegate" {
        return None;
    }
    if name.starts_with("host.") || name.starts_with("billing.") {
        return Some(Scope::ToolsWrite);
    }
    Some(Scope::ToolsCall)
}

/// Parse the stored `scopes_json`; an unreadable value fails closed to no
/// scopes at all.
pub fn parse_json(raw: &str) -> Vec<Scope> {
    serde_json::from_str::<Vec<String>>(raw)
        .map(|v| v.iter().filter_map(|s| Scope::parse(s)).collect())
        .unwrap_or_default()
}

/// The last name segments of a verb that only reads.
const READ_VERB_SEGMENTS: &[&str] = &[
    "list", "get", "status", "describe", "search", "inspect", "show", "history", "stats", "whoami", "explain",
    "quickstart", "scopes", "usage", "preview", "read", "export", "lookup", "doctor", "docs", "help",
];

/// The one scope a listed verb declares in `tools/list` as `_meta.scope`.
///
/// This is documentation of the verb's authority, not a new check: a key
/// caller is unrestricted and an OAuth token is still gated by `mcp` on
/// control-plane verbs, so no existing caller's outcome moves.
pub fn verb_scope(name: &str) -> Scope {
    let bare = name.rsplit_once('.').map_or(name, |(_, last)| last);
    if ALWAYS_LISTED.contains(&name) {
        return Scope::Read;
    }
    if name.starts_with("host.key.") || name.starts_with("host.keys.") {
        return Scope::KeysManage;
    }
    if name.starts_with("host.secret") {
        return Scope::SecretsWrite;
    }
    if name.starts_with("host.state.") {
        return if READ_VERB_SEGMENTS.contains(&bare) { Scope::StateRead } else { Scope::StateWrite };
    }
    if READ_VERB_SEGMENTS.contains(&bare) { Scope::ToolsRead } else { Scope::ToolsWrite }
}

/// The scope words a call to the wire name `name` needs when it is not one
/// of the caller's own published tools: the registry scope of a `host.*` /
/// `billing.*` verb (`mcp` implies it), and `mcp` for anything else.
pub fn control_plane_requirement(name: &str) -> Vec<String> {
    if name.starts_with("host.") || name.starts_with("billing.") {
        vec![verb_scope(name).as_str().to_string()]
    } else {
        vec![Scope::Mcp.as_str().to_string()]
    }
}
