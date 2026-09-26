-- compat: previous -- three wholly new tables; an old release simply never
-- queries them, no existing row's shape changes, no existing statement's
-- result set changes (PRD-mcphost-migration-safety requirement 4, same
-- convention 0040's own compat note already follows).
-- mcphost 0041_status_feed: PRD-mcphost-status-feed requirement 1.
--
-- `status_samples`: one row per probe tick (self, `source: "self"`) or
-- external post (`admin.status.sample`, `source` caller-supplied, e.g.
-- `"deploy-probe"`). `component` is one of the four fixed names (`mcp`,
-- `exec`, `billing`, `claim`); pruned past 90 days by the daily prune,
-- independently of `status_daily`, which keeps every rolled-up day
-- forever (uptime history must survive the raw-sample prune, AC7).
CREATE TABLE IF NOT EXISTS status_samples (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    component   TEXT NOT NULL,
    ts          INTEGER NOT NULL,
    ok          INTEGER NOT NULL,
    latency_ms  INTEGER NOT NULL,
    source      TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_status_samples_component_ts ON status_samples(component, ts);
CREATE INDEX IF NOT EXISTS idx_status_samples_ts ON status_samples(ts);

-- `status_daily`: one row per `(component, day)`, `day` an RFC 3339 UTC
-- date (`"YYYY-MM-DD"`, sorts lexicographically like `AgeColumn::Rfc3339Text`
-- elsewhere in this crate). Never pruned -- `/status.json`'s 90-day uptime
-- and `?days=N` history read this, not the raw samples.
CREATE TABLE IF NOT EXISTS status_daily (
    component        TEXT NOT NULL,
    day              TEXT NOT NULL,
    ok_samples       INTEGER NOT NULL,
    total_samples    INTEGER NOT NULL,
    p95_latency_ms   INTEGER NOT NULL,
    PRIMARY KEY (component, day)
);

-- `incidents`: one row per operator-opened (`admin.incident.open`) or
-- auto-opened (requirement 6, `status.component_down`) incident.
-- `components_json` is a JSON array of component names; `timeline_json` is
-- a JSON array of `{ts, type, message}` entries, appended to by
-- `admin.incident.update`/`admin.incident.close` and by the auto-close
-- path. `auto` distinguishes an incident this host opened on its own
-- (auto-closed after 10 consecutive ok samples) from an operator-opened
-- one (only ever closed by `admin.incident.close`).
CREATE TABLE IF NOT EXISTS incidents (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    title            TEXT NOT NULL,
    impact           TEXT NOT NULL,
    components_json  TEXT NOT NULL,
    opened_at        INTEGER NOT NULL,
    closed_at        INTEGER,
    timeline_json    TEXT NOT NULL,
    auto             INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_incidents_opened_at ON incidents(opened_at);
CREATE INDEX IF NOT EXISTS idx_incidents_closed_at ON incidents(closed_at);
