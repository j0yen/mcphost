-- compat: previous -- two wholly new tables; an old release simply never
-- queries them (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0058_row_policies: PRD-mcphost-row-policy requirement 1.
--
-- `row_policies` is a tenant's policy for one target -- a declared table
-- (`target_kind = 'table'`, `target_value` the table name) or a docs
-- prefix (`target_kind = 'doc_prefix'`, `target_value` the prefix) -- one
-- row per `(tenant_id, target_kind, target_value)`, `host.policy.set`
-- upserting in place and bumping `version`. `rule_json` is the serialized
-- `Vec<rowpolicy::PolicyRule>` (`src/rowpolicy/policy.rs`); kept as opaque
-- JSON rather than normalized columns since a rule's `value` is itself a
-- tagged union (`Literal` or `Attr`) `compile()` alone interprets --
-- nothing here ever reads or filters on `rule_json`'s contents in SQL.
CREATE TABLE IF NOT EXISTS row_policies (
    id            INTEGER PRIMARY KEY,
    tenant_id     INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    version       INTEGER NOT NULL,
    target_kind   TEXT NOT NULL,
    target_value  TEXT NOT NULL,
    rule_json     TEXT NOT NULL,
    created_unix  INTEGER NOT NULL,
    UNIQUE(tenant_id, target_kind, target_value)
);

-- `end_user_attributes` is the tenant-written attribute set `compile()`
-- resolves an `Attr(name)` rule value against for a given subject
-- (requirement 1: "End-user attributes come from a new
-- end_user_attributes table the tenant writes"). One row per
-- `(tenant_id, subject, attr)`; `value` is a JSON-encoded scalar or array
-- so an attribute can itself be a set (e.g. `roles: ["hr", "finance"]`)
-- without a second table.
CREATE TABLE IF NOT EXISTS end_user_attributes (
    tenant_id  INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    subject    TEXT NOT NULL,
    attr       TEXT NOT NULL,
    value      TEXT NOT NULL,
    PRIMARY KEY (tenant_id, subject, attr)
);
