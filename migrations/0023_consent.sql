-- compat: previous -- three wholly new tables (CREATE TABLE IF NOT EXISTS,
-- same precedent as migrations 0011/0014/0015/0017/0019/0020/0021) plus one
-- additive `messages.urgent` column (ALTER TABLE ADD COLUMN, same shape
-- migration 0022's `refused_json` already uses) an old release simply never
-- queries; no existing table's shape changes.
-- mcphost 0023_consent: PRD-mcphost-agent-consent requirements 1, 5, 6.
--
-- `contacts` / `contact_requests` / `muted` -- consent state layered on top
-- of migration 0021's `contact_policy`/`blocks`: a `contacts`-mode
-- recipient now actually distinguishes "no relationship yet" from "mutual,
-- accepted" instead of behaving as `closed` (0021's own note, `resolve_
-- message_recipient`'s doc comment). Cascade shape: every tenant-id column
-- below is ON DELETE CASCADE, same as `thread_participants`/
-- `message_receipts`/`blocks` in migration 0021 -- a deleted tenant's
-- contacts, requests (either direction) and mutes vanish with it (AC9).
--
-- `contacts(tenant_id, contact_tenant_id)`: an accepted pair is stored as
-- TWO rows, one per direction, so "does A see B in
-- `host.agent.contacts()`" is a plain `WHERE tenant_id = ?` lookup from
-- either side, no OR'd self-join -- `host.agent.contact_accept` inserts
-- both rows in the same transaction that flips `contact_requests.status`
-- to `accepted`.
--
-- `contact_requests`: `UNIQUE(from_tenant_id, to_tenant_id)` means a
-- denied request that later becomes requestable again (7-day cooldown,
-- requirement 3) is UPDATEd/UPSERTed back to `pending` in place rather
-- than inserted as a second row -- see `Db::contact_request`'s doc
-- comment. `status` (`pending`/`accepted`/`denied`/`expired`) has no CHECK
-- constraint, same as this crate's other enum-shaped TEXT columns (e.g.
-- `messages.source_class`) -- validated in Rust, never in SQL.
-- `created_unix_ms` backs the 30-day lazy-expiry read (requirement 11,
-- `consent::classify_existing_request`); `decided_unix_ms` backs the 7-day
-- deny cooldown read (requirement 3), both compared against
-- `state::now_unix_ms()` the same way migration 0021's own
-- `messages.created_unix_ms` backs `msgs_per_hour`. The
-- `idx_contact_requests_to` index is for `host.agent.contacts()`'s
-- incoming-request listing (requirement 3/9), the read this table exists
-- to serve fastest -- outgoing listing and the `UNIQUE` constraint's own
-- index already cover the `from_tenant_id` side.
--
-- `muted`: same unconditional, no-response-semantics shape as `blocks`
-- above (migration 0021) -- a mute is never a request the other party can
-- accept/deny, unlike `contacts`.
--
-- `messages.urgent` (requirement 6): `0`/`1`, defaulting every
-- pre-existing row (and every ordinary send) to non-urgent. Read by
-- `Db::msg_inbox`'s `unread_only` mute filter (requirement 6: urgent
-- bypasses the mute filter but never a block or `closed` policy) and
-- round-tripped through `MessageRow`/`message_row_json` so a caller can
-- tell. The (not-yet-built) wake PRD reads this same column to decide
-- whether a muted recipient's trigger fires (future work, out of this
-- PRD's scope -- see the deferred half of AC5).
CREATE TABLE IF NOT EXISTS contacts (
    tenant_id         INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    contact_tenant_id INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    accepted_at       TEXT NOT NULL,
    PRIMARY KEY (tenant_id, contact_tenant_id)
);

CREATE TABLE IF NOT EXISTS contact_requests (
    id               TEXT PRIMARY KEY,
    from_tenant_id   INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    to_tenant_id     INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    note             TEXT,
    status           TEXT NOT NULL DEFAULT 'pending',
    created_at       TEXT NOT NULL,
    created_unix_ms  INTEGER NOT NULL,
    decided_at       TEXT,
    decided_unix_ms  INTEGER,
    UNIQUE(from_tenant_id, to_tenant_id)
);
CREATE INDEX IF NOT EXISTS idx_contact_requests_to ON contact_requests(to_tenant_id, status);

CREATE TABLE IF NOT EXISTS muted (
    tenant_id       INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    muted_tenant_id INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    created_at      TEXT NOT NULL,
    PRIMARY KEY (tenant_id, muted_tenant_id)
);

ALTER TABLE messages ADD COLUMN urgent INTEGER NOT NULL DEFAULT 0;
