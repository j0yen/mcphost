# PRD: mcphost-python-kind-plain-env — non-secret configuration without abusing the secret store

- Status: building
- Lane: redbaron 2026-09-13T03:27:51Z pid=498754 boot=c6865fd1-71c2-48cf-818e-5e1f2246b3fe
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- build_priority: normal
- build_version_bump: minor
- publish: j0yen/private
- test_prefix: plainenv
- Vision: visions/mcp-host.md
- Grounding: failure-derived — five-whys in visions/mcphost-fleet-infrastructure.md (2026-09-12 18:00Z AC6 chain); product gap at the deepest level
- Loop: mcphost-buildloop: satisfaction
- PM: Joe Yen
- Drafted: 2026-09-12
- Engineering target: extend `~/wintermute/mcphost` — the python kind's spec parsing and sandbox environment assembly (`src/kinds/python.rs`), publish validation, the tool descriptor surface (`host.tool_get`/`host.tool_test`), docs and llms.txt

## TL;DR

A python-kind tool's process receives only secrets in its environment. A tool that needs
plain configuration — an endpoint URL, a mode flag, a tenant identifier — has two bad
options: put a non-secret into the encrypted secret store (where `host.tool_test`
redaction hides it and rotation policy misapplies to it) or hardcode it in source (a
republish per environment change). The fleet hit this wall on 2026-09-12: the
mcphost-fleet adapter could not receive its non-secret settings and a workaround PRD
(fleet-adapter-secret-env-fallback) shipped the values through the secret store, with the
product gap noted but undrafted. This PRD adds a plain `env` map to the python spec:
validated names, bounded size, visible unredacted in `tool_test` output, injected beside
secrets, refused on collision.

## Problem statement

Tool authors — today the fleet's own adapters, tomorrow any agent publishing a tool that
talks to a configurable upstream — cannot pass a non-secret value into their tool's
runtime. The evidence is a shipped workaround: PRD-fleet-adapter-secret-env-fallback
(queued 2026-09-12) exists solely because the adapter's plain settings had no path into
the sandbox except the secret store, and the dream note that drafted it recorded
"product component noted, not drafted: mcphost-python-kind-plain-env" (dream-log
2026-09-12T18:00Z). The misuse has real costs: secrets are AES-encrypted rows with a
rotation story, `tool_test` redacts them by design, and an operator auditing the secret
store cannot tell credentials from configuration — so the one store whose hygiene
matters most fills with values that were never secret. The production tools table
already shows authors improvising around missing configuration: seven live tools carry
a literal `{{workspace_host}}` placeholder in their URL template (2026-09-09 audit),
a value that would naturally have been an env entry.

## Goals

- A python tool's spec can declare plain configuration that reaches its process
  environment, distinct from secrets in storage, in visibility, and in audit.
- `host.tool_test` shows plain env values in the rendered environment while secrets
  stay redacted — the difference is observable, not doctrinal.
- Collisions and abuse are refused at publish time with structured errors naming the
  offending key.

## Non-goals

- No change to the http kind (its URL and header templates already interpolate;
  template-level env for http is a separate decision).
- No per-call env overrides (configuration is per-tool, set at publish/update).
- No encryption of plain env values — that is what the secret store is for; the point
  is the distinction.
- No migration of existing secret-store misuse (the fleet adapter's fallback keeps
  working; its values can move to `env` by its own republish, not by this PRD).

## User stories

1. **Agent publishing a tool.** When my function needs the upstream base URL it should
   call, I want to declare it beside my source at publish, so changing environments is
   an update, not a code edit.
2. **Agent debugging with `tool_test`.** When my tool misbehaves, I want the dry run to
   show the exact plain configuration my process will see, so misconfiguration is
   visible in one call — while my API key stays redacted.
3. **Operator auditing secrets.** When I review the secret store, I want everything in
   it to actually be secret, so rotation and exposure review cover credentials, not
   endpoint URLs.

## Requirements

**P0**
1. The python spec accepts an optional `env` map of name → string value. Names match
   `^[A-Z][A-Z0-9_]{0,63}$`; reserved prefixes and names are refused (`MCPHOST_*`,
   `PATH`, `HOME`, `PYTHON*`, `LD_*`, and the sandbox's own variables), each with a
   structured error naming the key and the rule.
2. Bounds enforced at publish: at most 16 entries, at most 4 KiB total across names and
   values, values valid UTF-8 with no NUL; violations are structured errors naming the
   key and the bound.
3. A name colliding with an existing secret name for the same tool is refused at
   publish (and setting a secret whose name collides with a published env entry is
   refused symmetrically), so precedence never needs a rule.
4. At call time the map is injected into the tool process environment beside secrets;
   an env-only tool (no secrets) receives its values under the same sandbox profile.
5. `host.tool_test` renders the environment with plain env values shown verbatim and
   secret values redacted, in one listing that labels which is which; `host.tool_get`
   (or the descriptor surface that returns a tool's spec) returns the env map.
6. Docs: the python kind's descriptor text, quickstart, README, and llms.txt document
   `env`, its bounds, and the secret/env distinction in one sentence each.

**P1**
7. Tool update: whatever surface republish/update uses accepts a changed env map
   without source changes, taking effect on the next call (respecting the existing
   warm-pool invalidation rules so a stale pool never serves old values).
8. The audit/journal row for publish records env names (never values), so config
   changes are traceable without leaking contents.

**P2**
9. `admin.*` tool inspection shows env names and sizes per tool for operator review.

## Success metrics

| metric | baseline | target | method | timeframe |
|---|---|---|---|---|
| Non-secret values entering the secret store | the fleet adapter's settings; unmeasured beyond it | new integrations use `env` | fleet adapter republish + code review of next integration | first integration after ship |
| Misconfiguration visible in tool_test | secrets redacted, config invisible | plain env verbatim in dry run | plainenv ACs | on ship |
| Publish-time refusal clarity | serde-level messages | structured error naming key + rule | plainenv ACs | on ship |

## Technical considerations

- The sandbox environment assembly already merges secrets; `env` joins the same
  assembly point so bwrap/profile behavior is identical for both (no second injection
  path to keep consistent).
- Spec storage: the `tools.spec` column already carries the parsed spec JSON; `env`
  rides in it — no schema migration expected. If a migration is needed, it is additive
  and reversible.
- Error surface follows the structured `KindError` conventions
  (PRD-mcphost-publish-first-try lineage): machine-readable code, the offending key,
  a docs hint.
- The warm pool keys process reuse; a changed env map must invalidate the pooled
  process for that tool (P1-7) exactly as a source change does.

## Migration / compatibility

Existing tools have no `env` and behave unchanged. Existing misuse (secrets holding
plain values) keeps working; nothing is force-migrated. Protocol surface is additive:
older clients simply never send `env`.

## Open questions

| question | owner | due |
|---|---|---|
None — decided by operator 2026-09-12: http-kind env unification is deferred until the first agent uses env on a python tool; `tool_logs` stays silent about env at process start.

## Acceptance criteria

1. P0 — Given a python publish with `env: {"UPSTREAM_URL": "https://example.test"}`, When the tool is called, Then the process observes `UPSTREAM_URL` with that value and the call succeeds.
2. P0 — Given env names `MCPHOST_X`, `PATH`, and `lower_case`, When each is published, Then each publish is refused with a structured error naming the key and the violated rule.
3. P0 — Given an env map with 17 entries, and another totaling over 4 KiB, When published, Then each is refused with a structured error naming the bound; Given exactly 16 entries under 4 KiB, Then publish succeeds.
4. P0 — Given a tool with secret `API_KEY` set, When a publish declares env `API_KEY`, Then it is refused naming the collision; Given a tool published with env `MODE`, When `secret_set MODE` is attempted, Then it is refused symmetrically.
5. P0 — Given a tool with secret `API_KEY` and env `MODE=fast`, When `host.tool_test` runs, Then the rendered environment shows `MODE=fast` verbatim, `API_KEY` redacted, and each labeled as env or secret.
6. P0 — Given a tool with env and no secrets, When called under the sandbox, Then the values are present and the sandbox profile is byte-identical to the secrets-only case apart from the variables themselves.
7. P0 — Given the descriptor and llms.txt after build, When grepped, Then `env` is documented with its bounds and the env-versus-secret distinction, and `host.tool_get` returns the env map for a tool that has one.
8. P1 — Given a published tool with a warm pooled process, When its env map is updated without source changes, Then the next call observes the new values (the pooled process was invalidated) and no republish of source occurred.
9. P1 — Given a publish with env, When the journal/audit row is read, Then it records the env names and never the values.
10. P2 — Given an admin inspection of a tool with env, When invoked, Then it reports env names and total size, never values.
