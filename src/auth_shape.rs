//! PRD-mcphost-self-offboard-success-envelope: the closed set of ways a
//! `tools/call` can authenticate, and the closed set of lifecycle verbs
//! whose own effect disables the caller's tenant.
//!
//! Both are generated contracts, not prose: `soff_matrix` iterates
//! [`AuthShape::ALL`] and fails naming any shape it has no runner for, and
//! `handler.rs` consults [`lets_through_disabled`] at every resolver that
//! refuses a disabled tenant.

/// How a call proves who it is (`handler.rs` precedence: URL secret ->
/// `Authorization` header -> `tenant_key` argument).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthShape {
    /// `Authorization: Bearer <key>` only.
    Header,
    /// `tenant_key` argument only.
    Arg,
    /// Both of the above on the same call.
    Both,
    /// `/u/<secret>/mcp` path secret.
    Url,
}

impl AuthShape {
    pub const ALL: &'static [AuthShape] = &[AuthShape::Header, AuthShape::Arg, AuthShape::Both, AuthShape::Url];

    pub fn name(self) -> &'static str {
        match self {
            AuthShape::Header => "header",
            AuthShape::Arg => "arg",
            AuthShape::Both => "both",
            AuthShape::Url => "url",
        }
    }
}

/// Canonical verbs that disable the calling tenant as their own effect. A
/// call to one of these is let through the disabled-tenant refusal when the
/// tenant is already self-offboarded, so the verb itself can answer
/// (idempotently) instead of the auth layer erroring.
pub const LIFECYCLE_TERMINAL_VERBS: &[&str] = &["host.self.offboard"];

/// `disabled_reason` that `Db::self_offboard_tenant` records.
pub const SELF_OFFBOARD_REASON: &str = "self_offboard";

/// PRD-mcphost-synthetic-implicit-tenant-expiry: `disabled_reason` the idle
/// sweep records; refused exactly like [`SELF_OFFBOARD_REASON`].
pub const IDLE_EXPIRED_REASON: &str = "idle_expired";

/// `true` iff a tenant disabled for `reason` ended itself (offboard or idle
/// expiry), so its peer is free to start a fresh tenant.
pub fn frees_peer(reason: Option<&str>) -> bool {
    matches!(reason, Some(SELF_OFFBOARD_REASON | IDLE_EXPIRED_REASON))
}

/// `true` iff a disabled tenant (`disabled_reason`) may still authenticate
/// for `verb` (the name as sent -- an alias resolves to its canonical).
pub fn lets_through_disabled(verb: &str, disabled_reason: Option<&str>) -> bool {
    let canonical = crate::tool_aliases::resolve(verb).unwrap_or(verb);
    LIFECYCLE_TERMINAL_VERBS.contains(&canonical) && disabled_reason == Some(SELF_OFFBOARD_REASON)
}
