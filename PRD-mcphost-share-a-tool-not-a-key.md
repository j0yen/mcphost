# PRD: mcphost-share-a-tool-not-a-key — a team's agents call a paid API without ever holding the key

- Status: queued
- Lane: orch 2026-09-19T13:43:36.916457106+00:00 run=15
- build_target: shell
- build_into: /home/jsy/wintermute/mcphost
- build_priority: high
- publish: none
- Cited-tree: mcphost@f59b60b
- Vision: visions/mcphost-killer-apps.md
- Grounding: opportunity — visions/mcphost-killer-apps.md, leaf 3 (primitives behave as documented against prod)
- PM: Joe
- Drafted: 2026-09-18
- Engineering target: mcphost.dev operations, `www/llms.txt` recipe section, `examples/share-a-tool/` proof script, synthorg mcphost consumer corpus (RedBaron); zero mcphost code changes

## TL;DR

A team lead's agent publishes one `http` tool that wraps a paid API with the key stored by `host.secret_set` and referenced as `{{ secret.<name> }}` (interpolated by the host at run time, `src/kinds/infer.rs` and `src/handler.rs`), shares it with `host.tool_share visibility=group`, and adds teammates' tenants with `host.group.add`. Every teammate's agent can call the tool; none can read the key; `host.group.remove` revokes in one call. The recipe ships as an llms.txt section, a proof script that runs it against mcphost.dev with two fresh tenants, and a synthorg task. The alternative today is pasting the key into every agent's environment.

## Problem statement

A team running two or more agents against one paid API (WHO) has to give each agent the raw key (WHAT), because no hosted MCP offers delegated, revocable access to a tool whose secret stays with the owner (WHY). Consequence: every agent's environment is a key copy, rotation means touching every agent, and a leaked agent transcript leaks the account. mcphost already has the three primitives (secrets, group sharing, host-side interpolation) and no page that puts them together; the only worked example on the host is homeward (visions/mcp-host.md 2026-09-18 addendum).

**Failure under this seed:** no — opportunity; the primitives exist and are untested as one flow.

## Goals

- A stranger reads one section and has a delegated tool live in under 3 minutes on the free plan.
- The proof script asserts, against production, that the caller never sees the secret and that revocation takes effect on the next call.
- The recipe is counted: real tenants that complete it are tagged.

## Non-Goals

- Per-caller usage breakdown inside `host.usage` (per tenant today, `src/control.rs`); open question in the vision.
- Rate limits per caller on a shared tool.
- Any change to sharing, secrets, or groups code.

## User stories

1. *Team lead's agent* — I publish `stripe_balance` once with `{{ secret.stripe }}` and share it to group `finance`; my teammates' agents call it by name.
2. *Teammate's agent* — I call `stripe_balance` and get the balance; `host.tool_list` shows me the tool, `host.secret_list` shows me nothing of the owner's.
3. *Team lead* — I run `host.group.remove` for one teammate; their next call fails with a permission error and the rest still work.
4. *Joe* — `healthz` tells me how many real tenants finished this recipe this week.

## Requirements

**P0**
1. `www/llms.txt` gains a section "Share a tool, not a key" (under 60 lines) with the exact six calls: `host.secret_set`, `host.tool_publish` (kind http, header using `{{ secret.<name> }}`), `host.group.create`, `host.tool_share visibility=group`, `host.group.add`, caller `host.tool_call`.
2. `examples/share-a-tool/proof.sh` creates two fresh tenants against `MCPHOST_URL` (default https://mcphost.dev/mcp), runs the recipe against a mock upstream (`httpbin`-style echo of headers or the repo's own echo tool) so no real paid key is needed, and asserts: the caller's call succeeds; the caller's `host.secret_list` is empty; the echoed header carries the secret value only in the owner's own test call, never in the caller's response body; after `host.group.remove` the caller's next call returns a permission error; total wall time from first signup to revoked call is printed.
3. The proof tags both tenants' signup with `source=recipe:share-a-tool` using the existing `signup_events` source field (as PRD-homeward-mcp-tenant does for non-synthorg sources).
4. Free-plan fit: the recipe uses 1 tool, 1 secret (limit 2), under 20 calls.

**P1**
5. A synthorg consumer task for the recipe (RedBaron corpus), scored on completion and wall time.
6. The proof runs in the mcphost test suite as a Live-marked test that is skipped without `MCPHOST_LIVE=1`.

**P2**
7. A second variant of the section for `visibility=public` with a note on why group is the default.

Non-functional: proof completes in under 120 s wall against prod; no secret value appears in any log line the proof writes.

## Success metrics

| metric | baseline | target | method | timeframe |
|---|---|---|---|---|
| primary: real tenants completing the recipe | 0 | 5 | `signup_events` rows with `source=recipe:share-a-tool` and a shared-tool call | 30 days after llms.txt ships |
| secondary: signup → revoked-call wall time on the synthetic panel | none | p50 under 90 s | synthorg task ledger | first 20 runs |
| guardrail: secret value observed by a caller | n/a | 0 | proof assertion on every Live run | always |

## Technical considerations

- Interpolation happens host-side; the tool's schema excludes `secret.*` (`src/kinds/infer.rs`), so the caller's `tool_list` entry has no secret parameter.
- Group membership is checked at call time (`src/sharing.rs`), so revocation needs no republish.
- The proof needs a mock upstream that echoes headers; prefer the repo's own echo kind over an external site so the test does not depend on the internet beyond mcphost.dev.

## Migration / compatibility

None. Docs and examples only.

## Open questions

| question | owner | due |
|---|---|---|
| Whether the second tenant in the proof counts against the synthetic panel or as real for healthz | Joe | at build |

## Acceptance criteria

1. P0 — Given llms.txt, When the section is read, Then it lists the six calls in order with one argument block each and is under 60 lines.
2. P0 — Given two fresh tenants, When `proof.sh` runs against `MCPHOST_URL`, Then the caller's `host.tool_call` on the shared tool returns the upstream's response.
3. P0 — Given the caller tenant, When it calls `host.secret_list`, Then the list is empty and no response body to the caller contains the secret value.
4. P0 — Given `host.group.remove` for the caller, When the caller calls the tool again, Then the response is a permission error and the owner's own call still succeeds.
5. P0 — Given the proof completed, When `signup_events` is queried for both tenants, Then `source=recipe:share-a-tool`.
6. P0 — Given the proof runs against https://mcphost.dev/mcp, When it finishes, Then every assertion above passes and the printed wall time is under 120 s. (Live; evidence: proof stdout captured in the receipt)
7. P1 — Given the synthorg mcphost corpus, When the new task runs 5 times, Then 5 completions are recorded with wall time.
8. P1 — Given `MCPHOST_LIVE` unset, When the test suite runs, Then the Live test is skipped, not failed.
9. P2 — Given the public variant section, When read, Then it states why group visibility is the default.
