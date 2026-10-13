# chain_errors_20261008.jsonl — the 10-08 discovery chain's live call errors (real corpus for PRD-mcphost-feedback-themes-nightly AC 1)

Source: orch `~/.local/state/synthorg/runs/explore-20261008T102422Z/evidence/exploration/sessions/*/build_log.json`,
every `events[]` row with `error_code == "live_call_error"` (59 rows; the chain digest of 10-08 reports the same 59).
Fields per row: `tool` (always `host.tool_call`), `args.name` (the tenant tool that failed), `result` (the error text the
client saw), `client_text`, `failure_class`, `auth_mode`, `ledger_verified`, `session`, `t`. Extracted 2026-10-12 10:4x pm PDT
by the RedBaron operator seat; not a reconstruction. For AC 1: text = `result`, tool = `args.name`, code = `failure_class`.
