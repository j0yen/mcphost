-- compat: previous -- one wholly new table an old release never queries; no existing row or column changes.
-- mcphost 0120_sampling_usage: PRD-mcphost-sampling-from-tools R4.
--
-- One row per (tenant, UTC day): the tokens `mcphost.sampling.ask` spent on
-- the client's model that day and how many asks made them. The day is its
-- UTC midnight as unix seconds, the boundary every `*_per_day` ceiling uses.
-- Counts only; never a prompt or an answer.
CREATE TABLE IF NOT EXISTS sampling_usage (
    tenant_id INTEGER NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    day_start INTEGER NOT NULL,
    tokens    INTEGER NOT NULL DEFAULT 0,
    asks      INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (tenant_id, day_start)
);
