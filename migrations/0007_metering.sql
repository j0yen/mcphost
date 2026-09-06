-- compat: previous -- every new column/table has a default (or is simply
-- unread by a rolled-back binary), consistent with every prior migration's
-- "applies forward with defaults" contract (PRD-mcphost-migration-safety
-- requirement 4).
-- mcphost 0007_metering: PRD-mcphost-metered-overage P0 requirements.
--
-- `tenants.stripe_customer_id` is a dedicated, definitely-a-customer-id
-- column: `billing_ref` (migration 0006) already stores whichever of
-- `customer`/`subscription` a webhook happened to carry, but the metering
-- payload Stripe's meter-events endpoint wants is specifically the
-- customer id, so this column is only ever set from `event.object.customer`,
-- never a subscription fallback.
--
-- `meter_state` is the single-row high-water mark `mcphost billing
-- emit-meter` advances (only after a successful POST); `meter_events` is
-- the durable ledger of every accepted (or replayed) batch, mirroring
-- `billing_events`' shape/rationale from migration 0006.
ALTER TABLE tenants ADD COLUMN stripe_customer_id TEXT;

CREATE TABLE IF NOT EXISTS meter_state (
    id           INTEGER PRIMARY KEY CHECK (id = 1),
    last_call_id INTEGER NOT NULL DEFAULT 0,
    updated_at   TEXT
);
INSERT OR IGNORE INTO meter_state (id, last_call_id, updated_at) VALUES (1, 0, NULL);

CREATE TABLE IF NOT EXISTS meter_events (
    id            INTEGER PRIMARY KEY,
    batch_id      TEXT NOT NULL,
    tenant_id     INTEGER REFERENCES tenants(id) ON DELETE CASCADE,
    first_call_id INTEGER NOT NULL,
    last_call_id  INTEGER NOT NULL,
    count         INTEGER NOT NULL,
    mode          TEXT NOT NULL CHECK (mode IN ('sent','replay')),
    created_at    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_meter_events_tenant ON meter_events(tenant_id, id);
-- Replay detection (AC5): has this exact call span already been ledgered?
CREATE INDEX IF NOT EXISTS idx_meter_events_span ON meter_events(tenant_id, first_call_id, last_call_id);
