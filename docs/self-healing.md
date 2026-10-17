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
| `sandbox_pool_restart` | 2 | no | Restart the warm python sandbox pool when its workers wedge. |
| `load_shed` | 6 | no | Briefly refuse new anonymous work while host pressure is above threshold. |
| `rollback` | 1 | no | Return to the previous release; refused until its actuator lands. |
| `db_maintenance` | 1 | no | Optimise, empty the write-ahead log and reclaim free pages once a day at the quietest hour. |
| `log_rotate` | 1 | no | Sweep expired scratch and export files once a day and flag a journal vacuum. |
<!-- mcphost-heal-actions:end -->

## `sandbox_pool_restart`

A python child that stops answering (an in-flight call older than
`wedge_factor` x its wall-clock timeout) holds its pool slot. When the
`sandbox_pool` signal breaches, the host marks the pool `draining` (new calls
wait, and `sandbox_unavailable` carries the wait estimate if the drain
outlasts `drain_budget_s` + 5 s), kills the wedged children, waits up to
`drain_budget_s` (`heal.toml`, default 30) for healthy calls, then rebuilds the
pool. A call still running when the drain ends, and a call whose child was
killed, fails with the existing `tool_timeout` code plus three additive
`data` fields:

- `heal_action_id`: the ledger row id of the restart (`/incidents.json`, `host.incidents.list`).
- `retry_after_ms`: when to retry (3000).
- `hint`: `the host restarted its sandbox pool; retry once`.

A call that waited out a drain carries `queued_ms` in its result. A third
restart within an hour is `rate_limited` (the action allows 2) and opens an
`exec` incident (`status.component_down`). `/status.json.sandbox_pool` reads
`{size, wedged, state, last_restart}`. The killed children's stderr tails
(at most 2 KB each) are in `host.incidents.list` rows under
`after.stderr_tails` and never in `/incidents.json`.
