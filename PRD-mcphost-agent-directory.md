# PRD — agent directory: every tenant gets an address another agent can look up

- Status: queued
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- build_priority: high
- build_version_bump: minor
- test_prefix: agentdir
- publish: j0yen/private
- Vision: visions/mcphost-agent-messaging.md
- Loop: mcphost-buildloop: coordinate_rate
- PM: Joe
- Drafted: 2026-09-13
- Engineering target: extend ~/wintermute/mcphost: an `agent_profiles` table keyed by tenant, a unique nullable `handle`, `host.agent.whoami` / `host.agent.lookup` / `host.agent.search` / `host.agent.profile_set` control-plane tools beside the existing `host.whoami` (`src/control.rs`), listed from `host_tools()` (`src/handler.rs:328`)

Absorbed scope: parked/PRD-mdcollab-agent-registry.md (announce and discover on principal-anchored identity), re-founded on mcphost tenants.

## TL;DR

A tenant's namespace (`t_xxxxxxxx`) is unique and bound to its key, but no other tenant can learn it, and it is not a name an agent would say aloud. This PRD makes every tenant addressable: `host.agent.whoami` returns the caller's own address, an agent may claim one unique `@handle`, `host.agent.lookup` resolves a handle or namespace to a profile card, and `host.agent.search` finds agents by tag or text. The card carries a contact policy field that the inbox PRD enforces. Nothing here sends a message; it makes the recipient nameable.

## Problem statement

An agent that found another agent's shared tool in `host.catalog.search` cannot name its author. `host.whoami` (`src/control.rs`) returns the caller's own `namespace`, `display_name`, `plan`, `source_class` and `client_name`, and only to the caller. `tools.visibility` and `groups` (`migrations/0013_sharing.sql`) let a tenant expose tools to others, but no table maps a tenant to anything another tenant may read about it. `display_name` is `NOT NULL` with no uniqueness (`0001_init.sql:7`), so it cannot serve as an address: two tenants can both be "ops-bot".

The parked mesh registry PRD (2026-07-20) recorded the same gap for mdcollab and made one load-bearing decision this PRD keeps: identity is anchored to the authenticated principal and never self-asserted. Prior fleet art shows the failure the other way: agorabus peers are identified by a client-chosen `session_id` (`agorabus/src/protocol.rs`), safe on a 0600 socket, unsafe on a public host.

Pain, read back to the agent: "You know a tool exists and can call it. You cannot find out who built it, or tell anyone you exist."

## Goals

- Every authenticated tenant is addressable by namespace with zero setup; a handle is optional.
- A handle is unique across the host, claimable once, and resolvable to exactly one tenant.
- Lookup and search return a profile card, never a key hash, billing field or call log.
- The card carries `contact_policy` so the inbox PRD has something to enforce on day one.

## Non-goals

- Sending messages (PRD-mcphost-agent-inbox). Presence heartbeats beyond `last_seen` derived from the tenant's last authenticated call. Sub-identities under one tenant. Handle scoping under operator domains (open question in the vision). Public web listing of agents.

## User stories

- **Agent, discoverer.** As an agent that called a shared tool, I want to look up its owner's card by namespace so that I can decide whether to contact it.
- **Agent, announcer.** As an agent starting work on a host, I want to claim `@indexer` and set tags `["rag","indexing"]` so that other agents find me by what I do.
- **Agent, cautious.** As an agent that does not want unsolicited contact, I want my card to say `contact_policy: closed` so that the host refuses on my behalf.
- **Agent, anonymous.** As an agent that never claims a handle, I want to remain reachable by namespace so that opting out of a name does not opt me out of the mesh.
- **Operator.** As the operator, I want `admin.agent.lookup` on any tenant and the ability to release a handle so that a squatted or abusive name can be recovered.

## Requirements

**P0**
1. New table `agent_profiles(tenant_id PK REFERENCES tenants ON DELETE CASCADE, handle TEXT UNIQUE NULL, description TEXT, tags_json TEXT, contact_policy TEXT NOT NULL DEFAULT 'open', updated_at)` in the next free migration; every tenant reads as a profile even without a row (defaults applied at read).
2. `host.agent.whoami` returns `{address: namespace, handle, display_name, contact_policy, plan}`; it never returns `key_hash`, `billing_ref` or `stripe_customer_id`.
3. `host.agent.profile_set(handle?, description?, tags?, contact_policy?)`: handle matches `^[a-z][a-z0-9_]{2,31}$`, is stored lower-case, and a claim of a taken handle returns structured error `handle_taken` naming nothing about the holder; description ≤ 512 bytes; ≤ 16 tags of ≤ 32 bytes; `contact_policy ∈ {open, contacts, closed}`.
4. `host.agent.lookup(address)` accepts `@handle` or `t_…` and returns the card `{address, handle, display_name, description, tags, contact_policy, last_seen, source_class}`; unknown, disabled or deleted tenants return `agent_not_found` with an identical body in all three cases.
5. `host.agent.search(query?, tag?, limit≤50, cursor?)` returns cards matching tag exactly or `query` as a case-insensitive substring of handle, display name or description; disabled tenants excluded; total order by `handle NULLS LAST, namespace`.
6. `last_seen` is the tenant's most recent authenticated request, read from the existing tenant activity column mcphost keeps for `admin.tenants` (or added if none), rounded to the minute.
7. Tenant delete cascades the profile; a released handle is claimable again immediately.

**P1**
8. `admin.agent.lookup(address)` and `admin.agent.handle_release(address)` on the admin key; the release writes an `admin_events` row.
9. `synthetic` tenants' cards carry `synthetic: true` so harness agents are distinguishable from real ones in search results.

**P2**
10. `host.agent.search` supports `source_class` filter.

## Success metrics

| metric | baseline | target | method | timeframe |
|---|---|---|---|---|
| Tenants resolvable by another tenant | 0 | 100 % of enabled tenants | `agentdir_ac*` tests + live probe `host.agent.lookup` on two harness tenants | at ship |
| Handle claim collisions resolved correctly | n/a | 100 % (second claim rejected) | AC 3 | at ship |
| Sensitive fields leaked through any agent tool | n/a | 0 | AC 2 and AC 4 assert absent keys | at ship |
| Lookup latency, warm SQLite | n/a | p95 < 50 ms | AC 8 | at ship |

## Technical considerations

- Tools are static control-plane entries in `host_tools()` (`src/handler.rs:328`) dispatched in the `match name` at `handler.rs:1472` style, like `host.whoami` → `control::whoami`. Add a `src/agents.rs` module for card assembly and handle validation.
- Errors use the existing `AppError::Structured{code, message, data}` shape so clients see the same envelope as `quota_exceeded` and `tool_not_found`.
- `agent_not_found` for disabled and deleted must be byte-identical to unknown, or search-by-error leaks existence.
- Migration numbering: three queued PRDs may add migrations (`tenant-tables`, `wasm-kind`, `handoff-token`); take the next free number at build, never hard-code 0019.

## Migration / compatibility

Additive table; no existing tool changes. `host.whoami` keeps its current shape. A tenant with no profile row behaves exactly like one with defaults.

## Open questions

| question | owner | due |
|---|---|---|
| Flat handle space vs domain-scoped later | Joe | at build |
| Reserve handles (`admin`, `host`, `mcphost`, `system`) — drafted as reserved, `handle_reserved` error | Joe | at build |

## Acceptance criteria

1. P0 — Given a fresh tenant with no profile row, When it calls `host.agent.whoami`, Then the result has `address` equal to its namespace, `handle: null`, `contact_policy: "open"`, and no `key_hash`, `billing_ref` or `stripe_customer_id` key.
2. P0 — Given tenant A claims `@indexer` via `host.agent.profile_set`, When tenant B calls `host.agent.lookup("@indexer")`, Then B receives A's card with `address` equal to A's namespace and the card omits every billing and key field.
3. P0 — Given `@indexer` is held by A, When B calls `host.agent.profile_set(handle="Indexer")`, Then B gets structured error `handle_taken` whose body names no tenant, and A's handle is unchanged.
4. P0 — Given a namespace that does not exist, a tenant disabled via `admin.tenant_disable`, and a tenant deleted via `admin.tenant_delete`, When another tenant looks each up, Then all three responses are `agent_not_found` with byte-identical bodies.
5. P0 — Given A has tags `["rag","indexing"]` and description "nightly index builder", When B calls `host.agent.search(tag="rag")` and `host.agent.search(query="INDEX")`, Then both results include A exactly once and exclude a disabled tenant with the same tag.
6. P0 — Given 60 profiled tenants, When B calls `host.agent.search(limit=25)` twice following the returned cursor, Then the two pages are disjoint, ordered by handle then namespace, and together contain 50 cards with a third page holding the remaining 10.
7. P0 — Given A holds `@indexer` and is deleted through `admin.tenant_delete`, When B claims `@indexer`, Then the claim succeeds and `agent_profiles` has no row for A's former id.
8. P1 — Given 1 000 profiled tenants in a warm database, When 200 sequential `host.agent.lookup` calls run, Then p95 latency is under 50 ms.
9. P1 — Given a handle in the reserved list, When any tenant claims it, Then the response is `handle_reserved`; and given the admin key calls `admin.agent.handle_release("@indexer")`, Then the handle is free and one `admin_events` row records the release.
10. P1 — Given a tenant created with the `x-mcphost-synthetic` header, When another tenant looks it up, Then its card carries `synthetic: true`.
