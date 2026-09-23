# PRD: mcphost-python-dependency-policy — a tool's dependencies are pinned and audited at publish

- Status: queued
- Lane: orch 2026-09-22T22:12:30.986879866+00:00 run=130
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- build_priority: normal
- publish: j0yen/private
- Vision: visions/mcp-host.md
- Loop: mcphost-buildloop: satisfaction — accuracy: a publish that would fail later fails now with a reason
- Grounding: wwhtbt — visions/mcp-host.md addendum 2026-09-18, leaf 5: `MAX_REQUIREMENTS = 20` (`kinds/python.rs:114`), any PyPI name, no hashes, no advisory check; the sandbox supports `network: none` (`sandbox.rs:19`) but a tool with network plus `host.secret_set` is an exfiltration path through one package
- PM: Joe
- Drafted: 2026-09-18
- deferred_acs: [6]
- deferred_ac_reasons: {"6": "post-ship production measurement: its Given is prod after ship and its proof is the next synthetic-panel measure row in the trailer, which no pre-land worktree can produce. Paired by the first vibeloop measure row after this PRD lands (operator-authorized 2026-09-22 23:05Z, Joe via Claude). Not a mock; the code path is exercised by AC1-5,7-9."}
- Operator-authorization: 2026-09-22 23:05Z Joe via Claude, verbatim "defer AC6 to the post-ship measure row" — AC6 cannot pair before landing by construction; the coder lands AC1-5,7-9 paired and AC6 deferred with the reason above; the operator pairs AC6 from the first post-deploy measure row.
- Engineering target: mcphost `src/kinds/python.rs` (env build, `infer_python_requirements`), `src/plans.rs`, `src/handler.rs` (`host.tool_test` report), migration `tool_lock`

## TL;DR

A python tool declares up to twenty PyPI names and the host installs whatever resolves
today. Tomorrow's resolve differs, and a typo-squatted name with network access and a
stored secret is a data leak. At publish the host resolves requirements to a lockfile
with hashes, stores it with the tool version, runs an advisory check, applies a per-plan
package policy, and rebuilds envs from the lock, never from names.

## Problem statement

Joe hosts other people's code with other people's secrets. Evidence: `python.rs:114`
caps count only; `grep -niE 'allowlist|denylist|index-url' python.rs` finds nothing;
`sandbox.rs` isolates network only when the tool's config says `network: none`;
`host.secret_set` exists (23-tool registry). Consequence: an agent that mistypes
`requests` as `reqeusts` installs an arbitrary package; a tool whose transitive
dependency changes upstream stops working on the next env rebuild with no change on
the tenant's side.

## Goals

- Reproducible envs: a lockfile with hashes per tool version.
- A publish fails fast with the advisory or policy reason.
- Network access for python tools is an explicit, plan-gated choice.

## Non-goals

- A private package index or vendoring wheels.
- Scanning tool source for malware.

## User stories

- As an agent, `host.tool_publish` returns `lock: {packages: 7, hashes: true}` and my
  tool runs the same next month.
- As an agent with a vulnerable pin, publish fails with the advisory id and the fixed
  version.
- As the operator, `free` tools run `network: none` by default; `pro` may opt in.
- As an agent, `host.tool_test` shows the resolved lock and the advisory result.

## Requirements

P0
1. At publish, requirements resolve with `uv pip compile --generate-hashes` (uv is
   already the host's Python tool) inside the sandbox; the lock is stored in
   `tool_lock(tenant, name, version, lock_text, resolved_unix)`; env builds use
   `uv pip sync --require-hashes` from the lock, never from names.
2. An advisory check (`uv`'s audit or `pip-audit` on the lock, offline database
   refreshed daily by a timer) runs at publish; a known vulnerability with a fix
   available fails publish with `dependency_advisory: <id> <package> fix=<version>`;
   `MCPHOST_ADVISORY_MODE=warn` downgrades to a warning in the result.
3. Package policy per plan in `plans.rs`: a denylist (`MCPHOST_PKG_DENY`, default a
   short list of known-abuse names and any name Levenshtein-1 from the top-100 PyPI
   names unless it is that name), and `network_default: none` for `free`, `egress`
   allowed for `pro` only when the tool config says `network: egress`.
4. `host.tool_test` reports the lock summary and the advisory result.

P1
5. `host.tool_publish` accepts `requirements` as a lock text (`--require-hashes`
   format) so an agent can pin its own; the host validates hashes.
6. A daily re-audit of stored locks marks tools with new advisories in
   `host.tool_list` (`advisories: n`) without disabling them.

P2
7. `admin.usage` counts tools by network mode and by advisory state.

## Success metrics

| metric | baseline | target | method | timeframe |
|---|---|---|---|---|
| python tools with a hashed lock | 0 % | 100 % of new publishes | `tool_lock` rows | at ship |
| publishes failed by advisory | n/a | reported daily | journal | ongoing |
| env rebuilds that change resolved versions | unmeasured | 0 | lock diff on rebuild | 30 d |

## Technical considerations

- The env cache key (`python.rs:15`, `hash-of-requirements`) becomes the lock hash.
- The advisory database refresh needs egress from the host process, not the sandbox;
  timer on the box.
- `network: egress` for `pro` still passes through the sandbox's rlimits.

## Migration / compatibility

Existing tools keep their envs; the first republish locks them. A daily job may
pre-lock existing tools with `MCPHOST_LOCK_BACKFILL=1` (operator-run).

## Open questions

| question | owner | due |
|---|---|---|
| Fail vs warn on advisories with no fix available (assumed: warn) | Joe | at build |
| Initial denylist contents | Joe | at build |

## Acceptance criteria

1. P0 — Given `requirements: ["requests"]`, When published, Then a lock with hashes is stored and the env is built with `--require-hashes`.
2. P0 — Given a stored lock, When the env is rebuilt a day later with a newer upstream release available, Then the resolved versions equal the lock.
3. P0 — Given a requirement pinned to a version with a known advisory and a fix, When published with `MCPHOST_ADVISORY_MODE=fail`, Then publish fails naming the advisory and the fixed version.
4. P0 — Given a `free` tenant, When a python tool is published without `network`, Then it runs with `network: none` and an outbound connect from it fails.
5. P0 — Given `reqeusts` in requirements, When published, Then it fails with `dependency_policy: denied (near-name of requests)`.
6. P0 — Given prod after ship, When the synthetic panel's python tasks run, Then first-try publish rate for python tools is unchanged within the seed noise floor (proof: the next measure row in the trailer).
7. P1 — Given a lock text with hashes supplied as `requirements`, When published, Then the host validates and stores it without re-resolving.
8. P1 — Given a new advisory for a stored lock, When the daily re-audit runs, Then `host.tool_list` shows `advisories: 1` for that tool and the tool still runs.
9. P2 — Given `admin.usage`, When read, Then tools are counted by network mode and advisory state.
- iter_log: 2026-09-20T23:38:24.326663036+00:00 decision 13 answered: re-admit: box env fixed 09-20 7:40 pm (sccache path, uv on PATH, shellcheck+bats); daemon restarted after binary swap
- iter_log: 2026-09-21T06:16:23.468611630+00:00 decision 16 answered: re-admit: box env fixed 09-20 (sccache/uv/shellcheck/bats); window reset fix 7869bf2 installed
- iter_log: 2026-09-22T15:06:32.624360324+00:00 decision 26 answered: re-admit on wm-build 0.8.20: rebuild from current main; prior run died of the daemon prds-pull/tmpdir bugs, not of this question
- iter_log: 2026-09-22T18:51:20.734030661+00:00 decision 28 answered: prod mcphost-1 redeployed 0.57.0->v0.59.3 on 2026-09-22 via mcphost-deploy (backup + migrate-incompatible, authorized by Joe); infra loop resolved
- iter_log: 2026-09-22T22:09:48.787768411+00:00 decision 37 answered: fresh run from current main: run 124 was already released (unpaired AC6); the PRD now defers AC6 to the post-ship measure row with operator authorization, so re-admit as a new run with a new worktree
