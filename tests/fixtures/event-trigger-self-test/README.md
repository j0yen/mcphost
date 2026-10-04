# AC11 fixtures — the real pre-feature truth-tier nightly

PRD-mcphost-event-trigger-self-test AC11 is deferred (operator-provisioned:
a nightly `synthorg consume --tier truth` run on orch, against a prod
deployment that carries this branch). These two files are the artifacts of
the last nightly that ran **without** this branch, so the bar the live check
applies (`ac11_verdict` in
`tests/mcphost_event_trigger_self_test_ac11_truth_tier_persona_reaches_event_run.rs`)
can be exercised on a real session on every `cargo test` — and pinned to
fail on it.

| file | source | selection |
|---|---|---|
| `ledger-20260929T200655Z-integration-specialist-02.json` | `~/repos/synthorg/runs/truth-tier-20260929T200655Z/ledger.jsonl` (sha256 `1d3a6892749360648271dba26824df03dcd13d040453b8348475f25a290a0827`) | the one row whose `task_id` is `integration-specialist-github-push-webhook-handler`, re-emitted with sorted keys and 1-space indent; no value changed |
| `transcript-20260929T200655Z-integration-specialist-02.jsonl` | `~/repos/synthorg/runs/mcp-host-capabilities-2026-09-09-consume/sessions/integration_specialist-panel_integration_specialist_02.jsonl` (sha256 `82ddb01726afb202315431942aeb1cfaade32e24d84e0f6d75aec373a8a9a966`, the `transcript_path` that ledger row names) | all 32 lines verbatim except turn 1, the `signup` call, whose response embeds a one-time claim URL; 31 lines kept, byte-for-byte |

Tenant keys, handoff tokens and secret values are already redacted at the
source by synthorg (`<tenant-key>`, `<redacted:N bytes>`); nothing was
redacted here.

What that night measured, unedited: `satisfaction 0.0`, three
`host_trigger_test` calls (each carrying a signature the persona computed
itself), and a final `done` run returning `{"commit_count": 3, ...}` where
the corpus gold reads `runs.last(trigger=event).result.payload.commits == 3`
— the PRD's own open question 4.
