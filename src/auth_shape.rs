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
/// tenant is already disabled by that verb (see [`DISABLED_PASSES`]), so the
/// verb itself can answer (idempotently) instead of the auth layer erroring.
pub const LIFECYCLE_TERMINAL_VERBS: &[&str] = &["host.self.offboard", "host.org.offboard"];

/// `(verb, disabled_reason)`: the verbs a disabled tenant's credential may
/// still reach, per the reason it is disabled for. A terminal verb passes for
/// the reason it itself records; `host.org.restore` enables rather than
/// disables, so it is its own row and passes for `org_offboard` only.
pub const DISABLED_PASSES: &[(&str, &str)] = &[
    ("host.self.offboard", SELF_OFFBOARD_REASON),
    ("host.org.offboard", ORG_OFFBOARD_REASON),
    ("host.org.restore", ORG_OFFBOARD_REASON),
];

/// `disabled_reason` that `Db::self_offboard_tenant` records.
pub const SELF_OFFBOARD_REASON: &str = "self_offboard";

/// `disabled_reason` that `Db::set_tenant_disabled` (`admin.tenant_disable`)
/// records.
pub const ADMIN_DISABLE_REASON: &str = "admin_disable";

/// PRD-mcphost-org-export-and-offboard-v2: `disabled_reason` of a tenant
/// disabled by `host.org.offboard`. Restorable by `host.org.restore`, never
/// purged, and (unlike [`SELF_OFFBOARD_REASON`]) does not free a peer.
pub const ORG_OFFBOARD_REASON: &str = "org_offboard";

/// PRD-mcphost-synthetic-implicit-tenant-expiry: `disabled_reason` the idle
/// sweep records; refused exactly like [`SELF_OFFBOARD_REASON`].
pub const IDLE_EXPIRED_REASON: &str = "idle_expired";

/// PRD-mcphost-external-unclaimed-tenant-expiry: the two `disabled_reason`
/// values of the external unclaimed policy. `EXPIRING` is phase 1 of the
/// invalidation (marked, teardown still running); `EXPIRED` is terminal and
/// the only one a claim restores or the purge deletes.
pub const UNCLAIMED_EXPIRING_REASON: &str = "unclaimed_expiring";
pub const UNCLAIMED_EXPIRED_REASON: &str = "unclaimed_expired";

/// Every value written to `tenants.disabled_reason` (the column is plain
/// TEXT, so this list is its only schema). The writers spell these through
/// the constants above, `host.admin.tenants` accepts exactly these as its
/// `disabled_reason` filter, and `orgx_ac02` scans `src/` for a write that
/// spells one outside this file.
pub const DISABLED_REASONS: &[&str] = &[
    SELF_OFFBOARD_REASON,
    ADMIN_DISABLE_REASON,
    IDLE_EXPIRED_REASON,
    UNCLAIMED_EXPIRING_REASON,
    UNCLAIMED_EXPIRED_REASON,
    ORG_OFFBOARD_REASON,
];

/// `true` iff a tenant disabled for `reason` ended itself (offboard, idle
/// expiry or the unclaimed policy), so its peer is free to start a fresh tenant.
pub fn frees_peer(reason: Option<&str>) -> bool {
    matches!(
        reason,
        Some(
            SELF_OFFBOARD_REASON
                | IDLE_EXPIRED_REASON
                | UNCLAIMED_EXPIRING_REASON
                | UNCLAIMED_EXPIRED_REASON
        )
    )
}

/// `true` iff a disabled tenant (`disabled_reason`) may still authenticate
/// for `verb` (the name as sent -- an alias resolves to its canonical).
pub fn lets_through_disabled(verb: &str, disabled_reason: Option<&str>) -> bool {
    let canonical = crate::tool_aliases::resolve(verb).unwrap_or(verb);
    DISABLED_PASSES.iter().any(|(v, reason)| *v == canonical && disabled_reason == Some(*reason))
}
