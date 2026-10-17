-- compat: previous -- two wholly new tables an old release never queries, plus
-- a rebuilt `invites` (CHECK widened to admit `kind = 'org'`, one nullable
-- `org_id` column); every existing row is carried forward unchanged.
-- mcphost 0116_orgs: PRD-mcphost-org-model requirements 1 and 4.
--
-- `orgs`: one row per team. `org_members`: a tenant's membership, a join row
-- and nothing else (no tenant column is rewritten). `tenant_id UNIQUE` is
-- "a tenant belongs to at most one org"; `removed_unix` keeps history and
-- the app treats a NULL `removed_unix` as live. The `role` CHECK list is
-- the `org::ROLES` const; tests/orgm_ac03 asserts they cannot drift.
CREATE TABLE IF NOT EXISTS orgs (
    id              INTEGER PRIMARY KEY,
    name            TEXT NOT NULL UNIQUE,
    owner_tenant_id INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    created_unix    INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS org_members (
    org_id       INTEGER NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    tenant_id    INTEGER NOT NULL UNIQUE REFERENCES tenants(id) ON DELETE CASCADE,
    role         TEXT NOT NULL CHECK (role IN ('owner', 'member')),
    joined_unix  INTEGER NOT NULL,
    removed_unix INTEGER,
    PRIMARY KEY (org_id, tenant_id)
);

CREATE INDEX IF NOT EXISTS idx_org_members_org ON org_members(org_id);

-- `invites` rebuild (CHECK cannot be altered in place). The Rust migration
-- wrapper (`Db::migrate_0116_orgs`) turns `foreign_keys` OFF around this
-- batch so DROP TABLE does not cascade into `invite_joins`.
CREATE TABLE invites_new (
    id                INTEGER PRIMARY KEY,
    code_hash         TEXT NOT NULL UNIQUE,
    inviter_tenant_id INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    kind              TEXT NOT NULL DEFAULT 'standard' CHECK (kind IN ('standard', 'standing', 'org')),
    share_json        TEXT NOT NULL DEFAULT '[]',
    max_uses          INTEGER,
    uses              INTEGER NOT NULL DEFAULT 0,
    caller_limit      INTEGER,
    expires_unix      INTEGER,
    revoked_unix      INTEGER,
    created_unix      INTEGER NOT NULL,
    code_plain        TEXT,
    org_id            INTEGER REFERENCES orgs(id) ON DELETE CASCADE
);
INSERT INTO invites_new (id, code_hash, inviter_tenant_id, kind, share_json, max_uses, uses,
                         caller_limit, expires_unix, revoked_unix, created_unix, code_plain)
    SELECT id, code_hash, inviter_tenant_id, kind, share_json, max_uses, uses,
           caller_limit, expires_unix, revoked_unix, created_unix, code_plain FROM invites;
DROP TABLE invites;
ALTER TABLE invites_new RENAME TO invites;
CREATE INDEX IF NOT EXISTS idx_invites_inviter ON invites(inviter_tenant_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_invites_standing_live
    ON invites(inviter_tenant_id)
    WHERE kind = 'standing' AND revoked_unix IS NULL;
