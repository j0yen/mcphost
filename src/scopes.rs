//! PRD-mcphost-key-delegate-child-keys R3: the scopes a delegated key
//! carries and the one rule that says which scope a verb needs.
//!
//! `mcp` is the primary key's scope and implies every other one, so a key
//! holding only `["mcp"]` (the primary, and every `host.key.mint` sub-key)
//! behaves exactly as it did before scopes existed.

use crate::subkeys::ALWAYS_LISTED;

/// The error codes a delegation produces, named once: the server returns them
/// from these constants and `delegation_recipe` / `llms_txt` print them from here, so a
/// documented code cannot drift from the one the server sends.
pub const SCOPE_ESCALATION: &str = "scope_escalation";
pub const KEY_REVOKED: &str = "key_revoked";

/// One authority a key can hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scope {
    /// Everything; the primary key's scope.
    Mcp,
    /// Call the tenant's published tools.
    ToolsCall,
    /// Call the `host.*` / `billing.*` management verbs (implies `tools:call`).
    ToolsWrite,
}

impl Scope {
    /// Every scope; the unit test in `delegation` enumerates this.
    pub const ALL: [Scope; 3] = [Scope::Mcp, Scope::ToolsCall, Scope::ToolsWrite];

    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Mcp => "mcp",
            Scope::ToolsCall => "tools:call",
            Scope::ToolsWrite => "tools:write",
        }
    }

    pub fn parse(s: &str) -> Option<Scope> {
        Scope::ALL.into_iter().find(|sc| sc.as_str() == s)
    }

    /// Whether holding `self` grants `other` (reflexive).
    pub fn implies(self, other: Scope) -> bool {
        match self {
            Scope::Mcp => true,
            Scope::ToolsWrite => matches!(other, Scope::ToolsWrite | Scope::ToolsCall),
            Scope::ToolsCall => other == Scope::ToolsCall,
        }
    }
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

/// One row of the scope catalog: a scope's wire name and every scope name that
/// implies it (itself included). Built from [`Scope::ALL`] by [`catalog`], so
/// the llms.txt Scopes table and the sub-agent recipe render from one list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopeRow {
    pub name: String,
    pub implied_by: Vec<String>,
}

impl ScopeRow {
    /// Whether a key holding the scope named `held` also holds this one.
    pub fn granted_by(&self, held: &str) -> bool {
        self.implied_by.iter().any(|n| n == held)
    }
}

/// Every scope in registry order.
pub fn catalog() -> Vec<ScopeRow> {
    Scope::ALL
        .into_iter()
        .map(|sc| ScopeRow {
            name: sc.as_str().to_string(),
            implied_by: Scope::ALL.into_iter().filter(|h| h.implies(sc)).map(|h| h.as_str().to_string()).collect(),
        })
        .collect()
}
