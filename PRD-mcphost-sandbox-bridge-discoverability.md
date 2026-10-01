# PRD — mcphost-sandbox-bridge-discoverability: every surface a python tool author sees names `import mcphost`

- Status: building
- Direct-build: laptop 2026-10-01 (session 6099ecef) — PR only, land via gate
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- Cited-tree: mcphost@8a4a88a
- build_priority: high
- build_version_bump: minor
- publish: j0yen/public
- test_prefix: bridgedisc
- Vision: visions/mcphost-fleet-board-findings.md
- Grounding: failure — fleet board dogfood 2026-09-30 23:30 PDT on tenant joe-test: the Sonnet coder wrote `import host`, got `No module named 'host'`, read `host.quickstart`'s note "a python tool's own code can read/write the same store" (`src/control.rs`, the `host.state.set` step note) and found no import name anywhere, then concluded the sandbox could not persist and fell back to a chain and to `network: "public"` (rejected: `network: requires the pro plan`). The bridge exists: `src/kinds/python.rs:2288-2292` (origin/main 8a4a88a) registers `sys.modules["mcphost"]`, `"mcphost.state"`, `"mcphost.table"`, `"mcphost.docs"`, `"mcphost.lineage"` (runner script line 1955: "A tool's own code does `import mcphost` / `from mcphost import state`"); the quickstart note is `src/control.rs:731`. Three coder rounds and ~95 tool calls instead of the quickstart's four.
- PM: Joe
- Drafted: 2026-10-01
- Engineering target: extend `~/wintermute/mcphost` — `src/control.rs` (quickstart builder: `starter_tool`, steps, `try_before_call`, limits), `src/kinds/python.rs` (spec field docs, import-error mapping, `host.spec_test` static check), `src/handler.rs` (python-kind and `host.tool_publish` descriptions in `tools/list`), `docs/kinds/python.md`, `www/llms.txt`, `plugin/skills/mcphost/SKILL.md`

## TL;DR

A python tool author learns the sandbox API from the first thing they read. `host.quickstart kind=python` prints the import line, the module table (`mcphost.state`, `mcphost.table`, `mcphost.docs`, `mcphost.call`, `mcphost.progress`) with one-line signatures, and a starter tool that appends to a table through the bridge. `tools/list` descriptions for `host.tool_publish` and the python kind carry the same line. A tool whose source imports `host`, `mcphost_sdk`, or any `mcphost.<x>` that is not registered fails `host.spec_test` and `host.tool_publish` with `unknown_import` naming the real module. A sandbox `ModuleNotFoundError` for those names maps to the same hint at run time. The quickstart's limits table says `network: "public"` is pro-only.

## Problem statement

An agent publishing its first python tool (persona: integration specialist / workflow orchestrator, projects/mcp-host.md) needs to persist or read tenant state from inside the tool. It cannot discover how: the only mention is a sentence in a quickstart step note that names no module, the starter tool (`text_stats`) uses no bridge, and the python kind's documented spec fields (`network`, `secrets`, `requirements`) say nothing about an in-process API. Guessing `import host` produces a bare Python traceback. The consequence on 2026-09-30 was two of six fleet-board steps needing a human relay and a false product finding ("no tool execution context can reach host.table") that survived one full build round. Every first-hour agent without a human reading the Rust runner script will hit the same wall.

## Goals

- An agent that reads only `host.quickstart kind=python` can write a tool that appends to a table on the first publish.
- A wrong import fails at publish time with the right module named, never at run time with a traceback.
- The documentation that teaches the import is executed, not asserted (Live AC against prod).

## Non-goals

- Adding new bridge modules (`mcphost.channel`, `mcphost.msg`) — PRD-mcphost-sandbox-channel-msg-bridge.
- Changing `network` plan gating — open question in the vision.
- Renaming any tool — PRD-mcphost-tool-naming-convention-and-aliases owns names; this PRD uses canonical names as that PRD defines them at landing.

## User stories

- As **an integration-specialist agent**, I call `host.quickstart kind=python` and see `from mcphost import table` with `table.append(name, rows)` so that my first tool persists without a second lookup.
- As **a workflow-orchestrator agent**, when I publish a tool that does `import host`, I get `unknown_import: no module 'host'; the sandbox API is 'import mcphost' (mcphost.state, mcphost.table, mcphost.docs)` so that I fix it before the webhook fires.
- As **a free-plan agent**, I see in the limits table that `network: "public"` requires pro so that I do not design around egress I cannot have.
- As **the operator**, I want the quickstart's python starter executed against prod nightly so that a bridge rename can never silently orphan the docs again.
- As **an agent reading `tools/list` only**, the `host.tool_publish` description's python paragraph names `import mcphost` so that even a client that never calls quickstart sees it.

## Requirements

**P0**
1. `host.quickstart kind=python` returns a new field `sandbox_api` : `{import: "import mcphost", modules: {"mcphost.state": [...signatures], "mcphost.table": [...], "mcphost.docs": [...]}, attrs: ["mcphost.call", "mcphost.progress"]}`, generated from the same registration list the runner script uses (one source of truth; a module added to the runner appears in quickstart without a second edit).
2. The python `starter_tool` becomes a bridge example: validates input, `mcphost.table.append` into a table named in the example, returns the bridge result; `test_call` matches. The echo/http/chain/wasm quickstarts keep their starters.
3. `host.spec_test` and `host.tool_publish` for kind python statically scan `source` for `import host`, `from host import`, `import mcphost_sdk`, and `mcphost.<name>` attribute use where `<name>` is not registered; failure is `unknown_import` with `data.hint` naming the registered modules. Dynamic imports are out of scope.
4. A sandbox run whose stderr ends in `ModuleNotFoundError: No module named 'host'` (or `mcphost_sdk`) returns the same `unknown_import` error class and hint instead of a raw traceback.
5. The quickstart `limits.plan` object gains `network_public: "pro"` (or the plan name that allows it) and the `try_before_call` table's python row says "no network by default".

**P1**
6. `tools/list` descriptions: `host.tool_publish` python paragraph and the python kind's spec-field help (`"network" => ...` block) mention `import mcphost` and the three modules.
7. `docs/kinds/python.md`, `www/llms.txt`, and `plugin/skills/mcphost/SKILL.md` carry the same module table, generated or checked from the one source (a `scripts/sandbox-api-doc-check.sh` that diffs the rendered table against the runner's registration list; exit 1 on drift).

**P2**
8. `host.quickstart kind=python` includes one `mcphost.state` example (`get`/`set`) beside the table starter.

**Non-functional:** quickstart response grows by at most 2 KB; the static import scan adds at most 5 ms to publish for a 64 KB source; no change to any existing tool's behavior.

## Success metrics

| Metric | Baseline (2026-09-30) | Target | Method | Timeframe |
|---|---|---|---|---|
| Primary: calls from first `host.quickstart kind=python` to first successful bridge write by a fresh tenant | ~95 (coder transcript, three rounds) | ≤ 6 | synthorg explore persona on prod, calls counted from `host.usage by=tool` | first nightly after landing |
| Secondary: publishes rejected `unknown_import` that would previously have run | 0 (not detected) | every `import host` source rejected | fixture publishes + prod probe | at landing + nightly |
| Guardrail: quickstart p95 latency | measured at landing | +≤ 5 ms | `host.usage` p95 on `host.quickstart` | 7 days |

## Technical considerations

- The runner script's module registration (`sys.modules["mcphost.*"] = ...` in `src/kinds/python.rs`) is the source of truth; extract the module/attr list into one Rust constant that both the runner-script template and `control.rs` read. Signatures come from the same constant (name, args, one-line doc).
- Static import scan: a line-based regex over `source` is sufficient (`^\s*(from|import)\s+host\b`, `mcphost\.([a-z_]+)`), no AST; false positives inside string literals are acceptable at P0 and noted.
- Run-time mapping: the python kind already classifies sandbox stderr into error classes; add one pattern.
- `host.quickstart` is unauthenticated for the signup step; `sandbox_api` is static and safe to show unauthenticated.

## Migration / compatibility

Additive fields only; existing quickstart consumers ignore `sandbox_api`. Existing published tools that import `host` do not exist (they could never have run); the static check cannot break a working tool. No schema change.

## Open questions

| Question | Owner | Due |
|---|---|---|
| Should the starter's table be auto-created on first publish (needs `mcphost.table.create` in the starter) or should quickstart tell the author to run `host.table.create` first? Drafted: starter calls `table.create` if missing. | Joe | 2026-10-08 |
| Should `mcphost.call` (call a sibling tool) appear in the starter as well? Drafted: listed in `attrs`, not in the starter. | Joe | 2026-10-08 |

## Acceptance criteria

1. P0 — Given a fresh tenant, When it calls `host.quickstart kind=python`, Then the response carries `sandbox_api.import == "import mcphost"` and `sandbox_api.modules` keys exactly equal to the runner script's registered `mcphost.*` modules, and a fixture that adds a module to the registration constant sees it in quickstart with no other edit.
2. P0 — Given the python `starter_tool` from quickstart published verbatim, When `host.tool_test` runs its `test_call`, Then the result contains a bridge `appended` count and `host.table.query` on the starter's table returns the row (or, after PRD-mcphost-dry-run-side-effects lands, the would-write report).
3. P0 — Given a python spec whose source contains `import host`, When `host.spec_test` or `host.tool_publish` runs, Then the error class is `unknown_import`, `data.hint` contains `import mcphost` and all three module names, and nothing is published.
4. P0 — Given a published python tool whose source does `import mcphost_sdk` inside a function (escaping the static scan), When it is called, Then the call fails `unknown_import` with the same hint and no raw traceback appears in `result`.
5. P0 — Given a free-plan tenant, When it calls `host.quickstart kind=python`, Then `limits.plan.network_public` names the plan that allows `network: "public"`, and the python row of `try_before_call` says network is off by default.
6. P1 — Given `tools/list`, When fetched, Then the `host.tool_publish` description and the python kind's field help each contain `import mcphost` and `mcphost.table`.
7. P1 — Given `scripts/sandbox-api-doc-check.sh` at the landing commit, When run, Then it exits 0 and reports `modules=<n> docs_in_sync=3`; given a fixture doc with one module removed, Then it exits 1 naming the missing module and file.
8. P0 — Given the landed binary deployed on prod (deploy journal line names the tag), When a fresh tenant created by the synthorg explore persona runs `host.quickstart kind=python` and publishes the returned starter verbatim, Then its first `host.tool_test` succeeds and `host.usage by=tool` for that tenant shows at most 6 calls between quickstart and the first successful bridge write (Live: prod tenant created by the probe, evidence = the probe's JSON report and `host.usage` output pasted into the PRD's evidence block)
