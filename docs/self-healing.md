# Self-healing: the golden signals

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
