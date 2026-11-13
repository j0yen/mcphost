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
| `db_maintenance` | 1 | no | Optimise, empty the write-ahead log and reclaim free pages once a day at the quietest hour. |
| `log_rotate` | 1 | no | Sweep expired scratch and export files once a day and flag a journal vacuum. |
<!-- mcphost-heal-actions:end -->

## Sandbox pool restart

When the `sandbox_pool` signal breaches (the ratio of wedged calls to pool
children stays at or above `red` for `for_s`), the host restarts its python
sandbox pool itself, inside the `sandbox_pool_restart` hourly limit above:

1. the pool goes `draining`; a new call waits (at most `drain_budget_s` + 5 s,
   then it answers `sandbox_unavailable` with `retry_after_s`);
2. calls older than `wedge_factor` x their timeout are killed;
3. healthy in-flight calls run for up to `drain_budget_s` (`heal.toml`,
   `[signals.sandbox_pool]`, default 30);
4. calls still running are cancelled;
5. the warm pool is emptied and the state returns to `ready`.

One `sandbox_pool_restart` row records it; `trigger` carries `signal`, `ratio`
and `wedged`, `before` and `after` carry `wedged`, `pool_size` and
`in_flight_count`. The wedged calls' `tenant/tool` keys and the killed
children's stderr tails (`after.stderr_tails`, at most 2 KB each) are private:
the admin-only incidents list returns them, `/incidents.json` omits them. With
`MCPHOST_HEAL_DRY_RUN=1` the row reads `would_run` and nothing is killed. When
the hourly limit is spent the row reads `rate_limited` and the host opens one
incident naming `exec` that it does not auto-close.

`/status.json.sandbox_pool` reads `{size, wedged, state: ready | draining,
last_restart: {at, outcome, heal_action_id} | null}`.

### `tool_timeout` after a restart

A call the restart killed or cancelled answers `tool_timeout` (code
unchanged) with three additive `data` keys:

| key | meaning |
|---|---|
| `heal_action_id` | the id of the `sandbox_pool_restart` ledger row (`null` if the row was not written within 2 s) |
| `retry_after_ms` | how long to wait before retrying |
| `hint` | says the host restarted its sandbox pool |
