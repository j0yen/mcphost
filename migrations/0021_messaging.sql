-- compat: previous -- five wholly new tables (CREATE TABLE IF NOT EXISTS,
-- same precedent as migrations 0011/0014/0015/0017/0019/0020); no existing
-- table's shape changes.
-- mcphost 0021_messaging: PRD-mcphost-agent-inbox requirement 1.
--
-- `threads` / `thread_participants` / `messages` / `message_receipts` /
-- `blocks` -- directed messages and threads between tenants, addressed
-- through the agent directory (migration 0020). Cascade shape:
--   - `threads.created_by`, `messages.from_tenant_id`: ON DELETE SET NULL
--     so a deleted sender's messages remain readable (requirement 10) --
--     `messages.from_address` is denormalized for exactly this reason.
--   - `thread_participants`, `message_receipts`, `blocks`: ON DELETE
--     CASCADE, same shape migration 0005 already gives `tools`/`secrets`/
--     `calls`/`logs` -- a deleted tenant's participations, receipts and
--     blocks vanish with it (requirement 10).
--
-- `messages.created_unix_ms` (technical considerations: "Cursor: encode
-- (created_at_unix_ms, id) as an opaque string" -- `messaging.rs` uses a
-- plain `"<unix_ms>.<id>"` string rather than base64, since this crate
-- carries no base64 dependency and `agents::search`'s own cursor is
-- likewise a plain decimal string, not base64) is the ordering key
-- `host.msg.inbox`'s cursor pages over; `created_at` (RFC3339) is kept
-- alongside for the same human-readable-plus-queryable split migration
-- 0005's `calls.started_at`/`started_unix` already uses.
--
-- `UNIQUE(thread_id, seq)`: `seq` allocation races are impossible in
-- practice -- this crate's whole `Db` is one shared connection behind one
-- `Mutex` (see `db.rs`'s `Db::with_conn`), so two "concurrent" async sends
-- already serialize through that lock before either touches SQLite; the
-- constraint stays as a second, structural guarantee (AC6: 1,000 messages
-- across 20 concurrent senders, no duplicates, no gaps).
--
-- `UNIQUE(from_tenant_id, dedupe_key)`: requirement 8's within-24h resend
-- returns the original `message_id` without a second insert (AC4);
-- `Db::msg_send`'s own doc comment covers how a key is freed after 24h
-- without ever deleting the original message it named (nulls that row's
-- `dedupe_key` instead, in the same transaction that reuses the key).
--
-- `message_receipts` is deliberately the per-recipient "is this row in my
-- inbox" store, not merely a read marker: a row is inserted at delivery
-- time (`read_at` NULL) for every *delivered* recipient (never the sender,
-- never a `refused` recipient), so `host.msg.inbox` is a plain join on it
-- (requirement 4's "excluding its own" falls out for free -- the sender
-- never gets a receipt for its own send) and `inbox_rows_max` (requirement
-- 7) is a plain `COUNT(*)` against it.
CREATE TABLE IF NOT EXISTS threads (
    id              TEXT PRIMARY KEY,
    created_by      INTEGER REFERENCES tenants(id) ON DELETE SET NULL,
    created_at      TEXT NOT NULL,
    created_unix_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS thread_participants (
    thread_id  TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
    tenant_id  INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    joined_at  TEXT NOT NULL,
    PRIMARY KEY (thread_id, tenant_id)
);
CREATE INDEX IF NOT EXISTS idx_thread_participants_tenant ON thread_participants(tenant_id, thread_id);

CREATE TABLE IF NOT EXISTS messages (
    id              TEXT PRIMARY KEY,
    thread_id       TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
    seq             INTEGER NOT NULL,
    from_tenant_id  INTEGER REFERENCES tenants(id) ON DELETE SET NULL,
    from_address    TEXT NOT NULL,
    body            TEXT NOT NULL,
    data_json       TEXT,
    in_reply_to     TEXT,
    dedupe_key      TEXT,
    synthetic       TEXT,
    source_class    TEXT NOT NULL,
    created_at      TEXT NOT NULL,
    created_unix_ms INTEGER NOT NULL,
    UNIQUE(thread_id, seq),
    UNIQUE(from_tenant_id, dedupe_key)
);
CREATE INDEX IF NOT EXISTS idx_messages_thread_seq ON messages(thread_id, seq);
CREATE INDEX IF NOT EXISTS idx_messages_from_created ON messages(from_tenant_id, created_unix_ms);

CREATE TABLE IF NOT EXISTS message_receipts (
    message_id  TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    tenant_id   INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    read_at     TEXT,
    PRIMARY KEY (message_id, tenant_id)
);
CREATE INDEX IF NOT EXISTS idx_message_receipts_tenant ON message_receipts(tenant_id, message_id);

CREATE TABLE IF NOT EXISTS blocks (
    tenant_id          INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    blocked_tenant_id  INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    created_at         TEXT NOT NULL,
    PRIMARY KEY (tenant_id, blocked_tenant_id)
);
