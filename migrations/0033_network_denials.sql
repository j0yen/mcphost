-- compat: previous -- a wholly new table; an old release simply never
-- queries it (PRD-mcphost-migration-safety requirement 4).
-- mcphost 0033_network_denials: PRD-mcphost-sandbox-egress-allowlist
-- requirement 4 (AC7): admin healthz's `network_denied.{publish_plan,
-- run_plan, run_no_proxy}` counters need a durable, timestamped event log
-- (same shape `signup_events`, migration 0001, already established for
-- `distinct_source_ips_24h`) so "the operator can see denials" (this PRD's
-- Goals) survives a redeploy, rather than an in-memory tally that resets on
-- every restart.
CREATE TABLE IF NOT EXISTS network_denials (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    reason       TEXT NOT NULL,
    created_unix INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_network_denials_reason_created
    ON network_denials(reason, created_unix);
