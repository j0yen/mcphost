-- compat: previous -- one additive `tenants.mesh_frozen_at` column
-- (ALTER TABLE ADD COLUMN) plus three wholly new tables (CREATE TABLE IF
-- NOT EXISTS, same precedent as migrations 0011/0014/0015/0017/0019/0020/
-- 0021/0024) an old release simply never queries; no existing table's
-- shape changes.
-- mcphost 0027_mesh_ops: PRD-mcphost-agent-mesh-ops requirements 1, 4, 5.
--
-- `tenants.mesh_frozen_at` (requirement 4): RFC3339, set by
-- `admin.mesh.freeze`, cleared by `admin.mesh.unfreeze` -- `NULL` (the
-- migration-time default for every existing row) is "not frozen". Checked
-- directly off the loaded `Tenant` row at the top of `host.msg.send`/
-- `reply`, `host.channel.post` and `Db::contact_request`, never touching
-- reads, acks, `host.msg.wait`, inbound delivery, triggers or any
-- non-messaging tool (requirement 4's own "does not touch tools, key,
-- reads or inbound delivery").
--
-- `channels` / `channel_posts` / `channel_cursors`: the minimal
-- `host.channel.open`/`host.channel.post` vertical slice this PRD's own
-- AC4 (freeze must cover `host.channel.post`) and AC7 (a deleted tenant's
-- opened channel, post and cursor must leave no trace) need to exist
-- against -- no dependency PRD has built `host.channel.*` yet (see this
-- PRD's own iter_log operator note). Deliberately the same shape as
-- migration 0021's `threads`/`messages`: `channels.created_by` and
-- `channel_posts.from_tenant_id` are `ON DELETE SET NULL` (a deleted
-- tenant's channel and posts stay readable, `from_address` denormalized
-- for exactly that reason, same as `messages.from_address`); `UNIQUE
-- (channel_id, seq)` mirrors `messages`' own per-thread sequence
-- allocation under this crate's single-connection-behind-one-mutex
-- guarantee (see 0021's own comment). `channel_cursors` is `ON DELETE
-- CASCADE` on both `channel_id` and `tenant_id`, same as
-- `thread_participants`/`message_receipts` -- a deleted tenant's cursor,
-- or a deleted channel's cursors, vanish with it (AC7).
ALTER TABLE tenants ADD COLUMN mesh_frozen_at TEXT;

CREATE TABLE IF NOT EXISTS channels (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL UNIQUE,
    created_by      INTEGER REFERENCES tenants(id) ON DELETE SET NULL,
    created_at      TEXT NOT NULL,
    created_unix_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS channel_posts (
    id              TEXT PRIMARY KEY,
    channel_id      TEXT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    seq             INTEGER NOT NULL,
    from_tenant_id  INTEGER REFERENCES tenants(id) ON DELETE SET NULL,
    from_address    TEXT NOT NULL,
    body            TEXT NOT NULL,
    data_json       TEXT,
    synthetic       TEXT,
    source_class    TEXT NOT NULL,
    created_at      TEXT NOT NULL,
    created_unix_ms INTEGER NOT NULL,
    UNIQUE(channel_id, seq)
);
CREATE INDEX IF NOT EXISTS idx_channel_posts_channel_created ON channel_posts(channel_id, created_unix_ms);

CREATE TABLE IF NOT EXISTS channel_cursors (
    channel_id  TEXT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    tenant_id   INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    seq         INTEGER NOT NULL,
    updated_at  TEXT NOT NULL,
    PRIMARY KEY (channel_id, tenant_id)
);
