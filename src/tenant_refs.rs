//! PRD-mcphost-synthetic-tenant-purge R1/R3: the columns by which one
//! tenant's row names another, written once. The purge reads this list to
//! count, per column and by the other tenant's class, every row of another
//! tenant that names the subject; to hold a subject a row of a tenant outside
//! the synthetic class names; and to say what a delete does to the rest
//! (`nulled`, `delete`, `cascade`, `set_null`). `tests/tenp_ac02_*` derives the
//! same set from the migrated schema and fails by table name when a table is
//! added without being classified here.

/// What the delete does to the row that names the subject.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The naming column is set to NULL (a nullable column; the row stays).
    Nulled,
    /// The row is deleted by an explicit statement (no foreign key).
    Delete,
    /// The row is deleted by `ON DELETE CASCADE` (a `NOT NULL` column).
    Cascade,
    /// The author column is set to NULL by the schema; no counterpart column.
    SetNull,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Nulled => "nulled",
            Outcome::Delete => "delete",
            Outcome::Cascade => "cascade",
            Outcome::SetNull => "set_null",
        }
    }
}

/// One column that names a tenant from a row another tenant owns.
pub struct CrossRef {
    /// `table.column`, or `parent.column/child.column` for a join relation
    /// (the subject owns the parent row, the child row's tenant column is the
    /// other side).
    pub key: &'static str,
    pub outcome: Outcome,
    /// `SELECT <other tenant id> ...` with one row per row naming the subject,
    /// binding `:id`. A row whose other side is NULL or the subject itself
    /// names nothing across tenants and is left out by the query.
    pub other_sql: &'static str,
}

/// Two-sided columns: a row owned by another tenant that names the subject.
pub const TWO_SIDED: &[CrossRef] = &[
    CrossRef {
        key: "calls.caller_tenant_id",
        outcome: Outcome::Nulled,
        other_sql: "SELECT tenant_id FROM calls WHERE caller_tenant_id = :id AND tenant_id <> :id",
    },
    CrossRef {
        key: "runs.caller_tenant_id",
        outcome: Outcome::Nulled,
        other_sql: "SELECT tenant_id FROM runs WHERE caller_tenant_id = :id AND tenant_id <> :id",
    },
    CrossRef {
        key: "tenants.invited_by_tenant_id",
        outcome: Outcome::Nulled,
        other_sql: "SELECT id FROM tenants WHERE invited_by_tenant_id = :id AND id <> :id",
    },
    CrossRef {
        key: "tenants.scratch_of",
        outcome: Outcome::Nulled,
        other_sql: "SELECT id FROM tenants WHERE scratch_of = :id AND id <> :id",
    },
    CrossRef {
        key: "usage_daily.caller_tenant_id",
        outcome: Outcome::Delete,
        other_sql: "SELECT tenant_id FROM usage_daily WHERE caller_tenant_id = :id AND tenant_id <> :id",
    },
    CrossRef {
        key: "invite_joins.inviter_tenant_id",
        outcome: Outcome::Delete,
        other_sql: "SELECT invitee_tenant_id FROM invite_joins WHERE inviter_tenant_id = :id AND invitee_tenant_id <> :id",
    },
    CrossRef {
        key: "invite_joins.invitee_tenant_id",
        outcome: Outcome::Cascade,
        other_sql: "SELECT inviter_tenant_id FROM invite_joins WHERE invitee_tenant_id = :id AND inviter_tenant_id <> :id",
    },
    CrossRef {
        key: "blocks.blocked_tenant_id",
        outcome: Outcome::Cascade,
        other_sql: "SELECT tenant_id FROM blocks WHERE blocked_tenant_id = :id AND tenant_id <> :id",
    },
    CrossRef {
        key: "contacts.contact_tenant_id",
        outcome: Outcome::Cascade,
        other_sql: "SELECT tenant_id FROM contacts WHERE contact_tenant_id = :id AND tenant_id <> :id",
    },
    CrossRef {
        key: "muted.muted_tenant_id",
        outcome: Outcome::Cascade,
        other_sql: "SELECT tenant_id FROM muted WHERE muted_tenant_id = :id AND tenant_id <> :id",
    },
    CrossRef {
        key: "tool_version_watermarks.caller_tenant_id",
        outcome: Outcome::Cascade,
        other_sql: "SELECT tenant_id FROM tool_version_watermarks WHERE caller_tenant_id = :id AND tenant_id <> :id",
    },
    CrossRef {
        key: "tool_caller_limits.caller_tenant_id",
        outcome: Outcome::Cascade,
        other_sql: "SELECT tenant_id FROM tool_caller_limits WHERE caller_tenant_id = :id AND tenant_id <> :id",
    },
    CrossRef {
        key: "contact_requests.from_tenant_id",
        outcome: Outcome::Cascade,
        other_sql: "SELECT to_tenant_id FROM contact_requests WHERE from_tenant_id = :id AND to_tenant_id <> :id",
    },
    CrossRef {
        key: "contact_requests.to_tenant_id",
        outcome: Outcome::Cascade,
        other_sql: "SELECT from_tenant_id FROM contact_requests WHERE to_tenant_id = :id AND from_tenant_id <> :id",
    },
    CrossRef {
        key: "tasks.poster_tenant_id",
        outcome: Outcome::Cascade,
        other_sql: "SELECT recipient_tenant_id FROM tasks WHERE poster_tenant_id = :id AND recipient_tenant_id <> :id",
    },
    CrossRef {
        key: "tasks.recipient_tenant_id",
        outcome: Outcome::Cascade,
        other_sql: "SELECT poster_tenant_id FROM tasks WHERE recipient_tenant_id = :id AND poster_tenant_id <> :id",
    },
    CrossRef {
        key: "groups.owner_tenant_id/group_members.member_tenant_id",
        outcome: Outcome::Cascade,
        other_sql: "SELECT m.member_tenant_id FROM groups g JOIN group_members m ON m.group_id = g.id \
                    WHERE g.owner_tenant_id = :id AND m.member_tenant_id <> :id",
    },
    CrossRef {
        key: "groups.owner_tenant_id/channels.created_by",
        outcome: Outcome::Cascade,
        other_sql: "SELECT c.created_by FROM groups g JOIN channels c ON c.group_id = g.id \
                    WHERE g.owner_tenant_id = :id AND c.created_by <> :id",
    },
    CrossRef {
        key: "groups.owner_tenant_id/channel_posts.from_tenant_id",
        outcome: Outcome::Cascade,
        other_sql: "SELECT p.from_tenant_id FROM groups g JOIN channels c ON c.group_id = g.id \
                    JOIN channel_posts p ON p.channel_id = c.id \
                    WHERE g.owner_tenant_id = :id AND p.from_tenant_id <> :id",
    },
    CrossRef {
        key: "groups.owner_tenant_id/channel_cursors.tenant_id",
        outcome: Outcome::Cascade,
        other_sql: "SELECT k.tenant_id FROM groups g JOIN channels c ON c.group_id = g.id \
                    JOIN channel_cursors k ON k.channel_id = c.id \
                    WHERE g.owner_tenant_id = :id AND k.tenant_id <> :id",
    },
    CrossRef {
        key: "orgs.owner_tenant_id/org_members.tenant_id",
        outcome: Outcome::Cascade,
        other_sql: "SELECT m.tenant_id FROM orgs o JOIN org_members m ON m.org_id = o.id \
                    WHERE o.owner_tenant_id = :id AND m.tenant_id <> :id",
    },
    CrossRef {
        key: "orgs.owner_tenant_id/invites.inviter_tenant_id",
        outcome: Outcome::Cascade,
        other_sql: "SELECT i.inviter_tenant_id FROM orgs o JOIN invites i ON i.org_id = o.id \
                    WHERE o.owner_tenant_id = :id AND i.inviter_tenant_id <> :id",
    },
    CrossRef {
        key: "orgs.owner_tenant_id/invite_joins.invitee_tenant_id",
        outcome: Outcome::Cascade,
        other_sql: "SELECT j.invitee_tenant_id FROM orgs o JOIN invites i ON i.org_id = o.id \
                    JOIN invite_joins j ON j.invite_id = i.id \
                    WHERE o.owner_tenant_id = :id AND j.invitee_tenant_id <> :id",
    },
    CrossRef {
        key: "invites.inviter_tenant_id/invite_joins.invitee_tenant_id",
        outcome: Outcome::Cascade,
        other_sql: "SELECT j.invitee_tenant_id FROM invites i JOIN invite_joins j ON j.invite_id = i.id \
                    WHERE i.inviter_tenant_id = :id AND j.invitee_tenant_id <> :id",
    },
];

/// Author columns the schema sets to NULL when the tenant goes: counted under
/// `cross_refs` with `other: null`, never held (`table`, `column`).
pub const SET_NULL: &[(&str, &str)] = &[
    ("channel_posts", "from_tenant_id"),
    ("channels", "created_by"),
    ("messages", "from_tenant_id"),
    ("threads", "created_by"),
    ("signup_events", "tenant_id"),
];

/// Structural-set tables whose child rows carry the parent's own tenant, so no
/// other tenant's row is reached (`src/notify.rs`, `src/authz.rs`, `src/calltrace.rs`
/// and `src/index.rs` write the parent's tenant into the child).
pub const SAME_TENANT: &[&str] = &["notify_receipts", "oauth_jti_denylist", "call_traces", "index_sources", "index_chunks"];

/// Nullable columns that name a tenant; set to NULL inside the delete
/// (`table`, `column`). `calls.caller_tenant_id` and `tenants.invited_by_tenant_id`
/// are `NO ACTION` foreign keys, the other two have no foreign key.
pub const NULLED: &[(&str, &str)] = &[
    ("calls", "caller_tenant_id"),
    ("runs", "caller_tenant_id"),
    ("tenants", "invited_by_tenant_id"),
    ("tenants", "scratch_of"),
];

/// `NOT NULL` columns with no foreign key: the rows naming the tenant are
/// deleted (`table`, `column`).
pub const DELETED_BY_COLUMN: &[(&str, &str)] = &[("usage_daily", "caller_tenant_id"), ("invite_joins", "inviter_tenant_id")];

/// Tables with a `tenant_id` column and no foreign key: explicit
/// `DELETE ... WHERE tenant_id = :id` (`doc_chunks_fts` is a standalone FTS5 table).
pub const NO_FK_TENANT_TABLES: &[&str] = &[
    "call_traces",
    "help_search_events",
    "index_chunks",
    "index_sources",
    "network_denials",
    "oauth_issuers",
    "doc_chunks_fts",
];

/// Every table a [`TWO_SIDED`] key names, parent and child alike.
pub fn two_sided_tables() -> std::collections::BTreeSet<&'static str> {
    let mut tables = std::collections::BTreeSet::new();
    for r in TWO_SIDED {
        for part in r.key.split('/') {
            if let Some((table, _)) = part.split_once('.') {
                tables.insert(table);
            }
        }
    }
    tables
}

/// SQL: the class of tenant alias `o` for a counterpart. Decided by the
/// non-disabled terms only: a claimed two-marker tenant, a label-only tenant,
/// an origin-only tenant, `external` and `unattributed` are all `other`.
pub const OTHER_CLASS_SQL: &str = "CASE WHEN o.origin = 'synthetic' AND o.synthetic IS NOT NULL \
    AND o.claimed_unix IS NULL AND o.owner_verified_at IS NULL AND o.paid_unix IS NULL AND o.billing_ref IS NULL \
    THEN CASE WHEN o.signup_source = 'implicit' THEN 'synthetic_implicit' ELSE 'synthetic_explicit' END \
    ELSE 'other' END";
