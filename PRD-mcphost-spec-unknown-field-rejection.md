# PRD — mcphost-spec-unknown-field-rejection: an unknown field in a spec or a control-plane argument is an error that names the known fields

- Status: building
- Direct-build: laptop 2026-10-01 (session 6099ecef) — PR only, land via gate
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- Cited-tree: mcphost@8a4a88a
- build_priority: medium
- build_version_bump: minor
- publish: j0yen/public
- test_prefix: unkfield
- Vision: visions/mcphost-fleet-board-findings.md
- Grounding: failure — fleet board dogfood 2026-09-30, coder report: "unknown spec fields (e.g. a fictitious `write_table`) are silently ignored rather than rejected — easy to think a feature exists when it's a no-op"; the same session passed `name:` to `host.trigger.set` (no such argument; see PRD-mcphost-trigger-set-idempotent) and it was dropped without a word. Source: no `deny_unknown_fields` or unknown-key check exists in `src/kinds/python.rs` or `src/kinds/mod.rs` (grep on origin/main 8a4a88a: none); spec parsing reads known keys and ignores the rest. The per-kind field help table (`python.rs:339`, `"network" => (...)`) already enumerates known fields for error messages, so the known-field list exists per kind.
- PM: Joe
- Drafted: 2026-10-01
- Engineering target: extend `~/wintermute/mcphost` — `src/kinds/*.rs` (spec parsers for echo, http, python, wasm, chain), `src/kinds/mod.rs` (shared unknown-field check and error), `src/handler.rs` (control-plane argument schemas; `tools/list` `inputSchema.additionalProperties`), `docs/kinds/*.md`

## TL;DR

`host.tool_publish` and `host.spec_test` reject a spec containing a key the kind does not define: error `unknown_spec_field` with `data.field`, `data.kind`, `data.known` (the kind's field list), and a nearest-match hint. Every `host.*` control-plane tool rejects unknown top-level arguments the same way (`unknown_argument`), and `tools/list` publishes `additionalProperties: false` so schema-aware clients catch it before the call. A no-op key can no longer pass for a feature.

## Problem statement

An agent writing a spec from memory, from a docs page for another kind, or from a hallucinated field name gets silent acceptance: the tool publishes, the test passes, and the behavior the field was supposed to add never happens. The agent then builds on a feature that does not exist. On 2026-09-30 a fictitious `write_table` key in a python spec published cleanly, and a `name` argument to `trigger.set` was dropped, producing a duplicate trigger. For an agent, a silent no-op is worse than an error: it cannot be debugged from the outputs, which is exactly the shape every finding in this vision shares (rhyme check, levels 6-8).

## Goals

- No spec or control-plane call succeeds while carrying a key the server does not understand.
- The error teaches the right field.
- Schema-aware clients see the rule in `tools/list`.

## Non-goals

- Validating values beyond what each kind already validates.
- Rejecting unknown keys inside a tool's own `schema` for its arguments (those belong to the tool author).
- Rejecting unknown fields inside `config_json` blobs that are opaque by design (e.g. provider-specific OAuth config) — those get an explicit `extra` sub-object instead.

## User stories

- As **a tool author**, publishing `{source, write_table: "runs"}` fails `unknown_spec_field: 'write_table' is not a python spec field; known: source, requirements, network, secrets, env, outputs, schema — did you mean to call mcphost.table.append in source?` so that I learn the real mechanism.
- As **an agent calling `host.trigger.set` with `name`** before the idempotence PRD lands, I get `unknown_argument: name` so that I do not believe I named the trigger.
- As **a schema-aware MCP client**, I see `additionalProperties: false` in every `host.*` inputSchema so that I validate locally.
- As **the operator**, I want a `scripts/spec-fields-doc-check.sh` that diffs each kind's known-field list against `docs/kinds/<kind>.md` so that docs and parser cannot drift.
- As **an agent with an existing published tool carrying an unknown key**, I am not broken at run time; the key is flagged at next publish only.

## Requirements

**P0**
1. Each kind exposes `known_spec_fields() -> &[&str]` (one list per kind, the same list the field-help table uses); `host.tool_publish` and `host.spec_test` compare the spec's top-level keys against it and fail `unknown_spec_field` on the first unknown, with `data.known` and a Levenshtein nearest-match `data.did_you_mean` when distance ≤ 2.
2. Every `host.*` and `billing.*` tool rejects unknown top-level argument keys with `unknown_argument`, `data.known` from its registered `inputSchema.properties`; `tenant_key` is always known.
3. `tools/list` sets `additionalProperties: false` on every control-plane `inputSchema`.
4. Known fields of the python kind at landing include `source`, `requirements`, `network`, `secrets`, `env`, `outputs`, `schema`, `timeout_s` (whatever the parser reads today — the PR enumerates by reading the parser, not this list).
5. For known-but-misplaced keys (a field valid for another kind), the error says which kind accepts it (`data.valid_for: ["http"]`).

**P1**
6. `scripts/spec-fields-doc-check.sh` exits 1 when a kind's docs page lists a field the parser lacks or vice versa; wired into the repo's existing lint step.
7. Chain specs: unknown keys inside a step object (`tool`, `args`, plus whatever chain-run-lineage adds) are rejected the same way, with `data.step`.

**P2**
8. `host.tool_test` on an already-published tool whose stored spec has unknown keys returns a `warnings: [unknown_spec_field ...]` list without failing, so existing tools get a nudge.

**Non-functional:** the check is O(keys); publish latency unchanged within measurement noise.

## Success metrics

| Metric | Baseline (2026-09-30) | Target | Method | Timeframe |
|---|---|---|---|---|
| Primary: publishes on prod that carry an unknown top-level spec key | unknown (at least 1 observed) | 0 accepted | nightly: scan stored specs vs known lists; count new since landing | nightly after landing |
| Secondary: control-plane calls with unknown arguments accepted | ≥ 1 observed (`trigger.set name`) | 0 | fixture over every registered tool | at landing |
| Guardrail: false rejections of previously valid specs | n/a | 0 in the fixture corpus of every built PRD's example spec | at landing |

## Technical considerations

- The error must be raised before any side effect (no partial publish).
- The known-field list is the parser's truth: derive it from the struct (serde field names) where the spec is a struct, to prevent a second hand-maintained list; where parsing is manual (`json!` lookups), the PR adds the list next to the parser with a unit test that every key the parser reads is in the list.
- Control-plane argument rejection could break clients that send extra metadata keys; the fixture corpus of every existing docs example and the synthorg journey harness calls must pass before landing (guardrail metric).
- Interaction: PRD-mcphost-sandbox-bridge-discoverability's `unknown_import` is the source-level sibling of this spec-level check; share the hint style.

## Migration / compatibility

Stored specs are not re-validated; only new publishes and calls are checked. A deprecation note in CHANGELOG lists the kinds and the known-field lists. Clients sending unknown control-plane arguments start failing; the synthorg harness runs before landing (guardrail).

## Open questions

| Question | Owner | Due |
|---|---|---|
| Should `host.tool_call` arguments to a *user* tool be checked against the tool's own `schema` with `additionalProperties: false` by default? Drafted: no, author's choice. | Joe | 2026-10-08 |

## Acceptance criteria

1. P0 — Given a python spec with an extra key `write_table`, When `host.tool_publish` or `host.spec_test` runs, Then the error class is `unknown_spec_field`, `data.field == "write_table"`, `data.known` lists the python fields, and no tool is published.
2. P0 — Given a python spec containing `url` (an http field), When published, Then `data.valid_for` contains `"http"`.
3. P0 — Given a spec key one edit away from a known field (`sourc`), When published, Then `data.did_you_mean == "source"`.
4. P0 — Given `host.trigger.set` called with an argument key the tool does not define, When dispatched, Then the error class is `unknown_argument` with `data.known` equal to the registered properties and no trigger is created.
5. P0 — Given `tools/list`, When fetched, Then every `host.*` and `billing.*` tool's `inputSchema.additionalProperties == false`.
6. P0 — Given the example spec from every `docs/kinds/*.md` page and from `host.quickstart` for each kind, When each is passed to `host.spec_test`, Then none fails `unknown_spec_field` (guardrail).
7. P1 — Given `scripts/spec-fields-doc-check.sh` at the landing commit, When run, Then it exits 0 and prints one `kind=<k> fields=<n> doc_in_sync=true` line per kind; given a fixture doc with an extra field, Then it exits 1 naming it.
8. P0 — Given the landed binary deployed on prod (deploy journal line names the tag) and tenant joe-test, When `host.spec_test(kind: python, spec: {source: "def main(a): return a", write_table: "runs"}, invocations: [{}])` is called, Then the response is the `unknown_spec_field` error with `data.field == "write_table"` and `host.tool_list` shows no new tool (Live: prod tenant joe-test, evidence = the error envelope and the tool_list output pasted into the PRD's evidence block)
