-- compat: previous -- two wholly new tables; an old release simply never
-- queries them (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0083_asks: PRD-mcphost-ask-user-decision.
--
-- `asks` is one question an agent handed its user (`host.ask.create`):
-- `options_json` is the 2..6 button labels, `status` moves
-- `pending -> answered | expired` exactly once (the answering UPDATE is
-- guarded by `status = 'pending'`, so the first answer wins), and
-- `ip_hash` is the answerer's hashed address -- never the address itself.
-- `ask_events` is the audit trail the agent can list: one `created` row, then
-- exactly one `answered` or `expired` row.
CREATE TABLE IF NOT EXISTS asks (
    id               TEXT PRIMARY KEY,
    tenant_id        INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    question         TEXT NOT NULL,
    options_json     TEXT NOT NULL,
    context          TEXT,
    status           TEXT NOT NULL DEFAULT 'pending',
    choice           TEXT,
    created_unix     INTEGER NOT NULL,
    expires_unix     INTEGER NOT NULL,
    answered_unix    INTEGER,
    ip_hash          TEXT,
    waited_total_ms  INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_asks_tenant_status ON asks(tenant_id, status, created_unix DESC);
CREATE TABLE IF NOT EXISTS ask_events (
    id        INTEGER PRIMARY KEY,
    ask_id    TEXT NOT NULL REFERENCES asks(id) ON DELETE CASCADE,
    kind      TEXT NOT NULL,
    at_unix   INTEGER NOT NULL,
    detail    TEXT
);
CREATE INDEX IF NOT EXISTS idx_ask_events_ask ON ask_events(ask_id, id);
