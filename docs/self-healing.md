# Self-healing

## The golden signals

The host samples five golden signals every 60 s (`src/signals.rs`) and grades
each against `heal.toml`. `/status.json.signals` publishes the latest value and
band; a signal red for `for_s` seconds emits one `SignalBreach`.

The table below is generated from `heal.toml` by
`scripts/heal-doc-check.sh --write`; edit the toml, never the table.

<!-- mcphost-heal-signals:start -->
| signal | amber | red | for_s | unit |
|---|---|---|---|---|
| `echo_p95_ms` | 1000 | 2000 | 300 | ms |
| `cold_start_p50_ms` | 1500 | 4000 | 300 | ms |
| `sqlite_write_ms` | 50 | 500 | 120 | ms |
| `disk_free_mb` | 4096 | 1024 | 60 | MiB |
| `sandbox_pool` | 0.25 | 0.5 | 120 | ratio |
<!-- mcphost-heal-signals:end -->

## Self-healing actions

The host may take a small, closed set of actions on itself. Each one is a
`HealAction` variant in `src/heal.rs`, runs only through `heal::execute`, is
rate-limited per hour, and leaves one row in the `incidents` ledger (`action`,
`trigger`, `before`, `after`, `outcome`, `duration_ms`, `dry_run`). No action
is destructive: none removes tenant data.

Set `MCPHOST_HEAL_DRY_RUN=1` to record `would_run` rows without running
anything; `/status.json` reports `heal.dry_run` accordingly.

Outcomes: `ok`, `failed`, `refused`, `would_run`, `rate_limited`.

The table below is generated from `src/heal.rs` by
`scripts/heal-doc-check.sh --write`; do not edit it by hand.

<!-- mcphost-heal-actions:start -->
| action | max per hour | destructive | summary |
|---|---|---|---|
| `sandbox_pool_restart` | 4 | no | Restart the warm python sandbox pool when its workers wedge. |
| `load_shed` | 6 | no | Briefly refuse new anonymous work while host pressure is above threshold. |
| `rollback` | 1 | no | Return to the previous release; refused until its actuator lands. |
| `db_maintenance` | 2 | no | Checkpoint the write-ahead log when it grows past its size threshold. |
| `log_rotate` | 2 | no | Rotate host logs when the log volume nears full. |
<!-- mcphost-heal-actions:end -->
