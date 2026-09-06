-- compat: previous -- PRD-mcphost-migration-safety requirement 4: every new
-- column has a default and the previous release simply never reads
-- `billing_events`, so a rolled-back binary runs unmodified against the
-- migrated schema.
-- mcphost 0006_billing: PRD-grand-loop-billing P0 requirements.
--
-- `tenants` gains three columns (a plan, when it started, and the
-- processor's own id for that tenant), all with defaults so an existing
-- tenant becomes `free` with no processor reference -- consistent with
-- every prior migration's "applies forward with defaults" contract
-- (PRD-mcphost-migration-safety requirement 4).
--
-- `billing_events` is the durable ledger `admin.billing_ledger` reads and
-- the `measure` command consumes without ever needing Stripe credentials
-- (PRD technical considerations): one row per handled (or ledger-only, or
-- mode-mismatched) webhook event, `ON DELETE CASCADE` consistent with
-- 0005's cascade-delete of every other tenant-owned table. `mode` is
-- `test`/`live`, taken from the event's own `livemode` flag, never from
-- the host's configured key -- that comparison is exactly what a
-- `.mode_mismatch`-suffixed event_type records (AC10).
ALTER TABLE tenants ADD COLUMN plan TEXT NOT NULL DEFAULT 'free';
ALTER TABLE tenants ADD COLUMN plan_since TEXT;
ALTER TABLE tenants ADD COLUMN billing_ref TEXT;

CREATE TABLE IF NOT EXISTS billing_events (
    id             INTEGER PRIMARY KEY,
    event_id       TEXT NOT NULL UNIQUE,
    event_type     TEXT NOT NULL,
    tenant_id      INTEGER REFERENCES tenants(id) ON DELETE CASCADE,
    plan           TEXT,
    amount_cents   INTEGER,
    currency       TEXT,
    mode           TEXT NOT NULL CHECK (mode IN ('test','live')),
    received_at    TEXT NOT NULL,
    payload_sha256 TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_billing_events_tenant ON billing_events(tenant_id, id);
CREATE INDEX IF NOT EXISTS idx_billing_events_received ON billing_events(received_at);
