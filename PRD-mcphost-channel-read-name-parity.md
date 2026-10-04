# PRD — mcphost-channel-read-name-parity: every channel verb resolves a channel the same way `post` does

- Status: queued
- Lane: orch 2026-10-01T21:59:37.620964857+00:00 run=333
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- Cited-tree: mcphost@8a4a88a
- build_priority: medium
- build_version_bump: patch
- publish: j0yen/public
- test_prefix: chanread
- Vision: visions/mcphost-fleet-board-findings.md
- Grounding: failure — fleet board dogfood 2026-09-30: `host.channel.open(name: "fleet")` returned id `01M3V2YTK8F0TF96KX6Q29T08Z`; `host.channel.post` on that id succeeded; `host.channel.read` on the same id failed `no channel found for that name or id` (reproduced three times). Source: `post` (`src/channels.rs:106`) tries the group-channel lookup and falls back to `SELECT id FROM channels WHERE id = ?1 OR name = ?1` (`src/db.rs:10772`, `:10859`); `read` (`channels.rs:359`) performs only the group lookup (`JOIN groups g ON g.id = c.group_id`), so a channel opened by name — a path `open` still documents as supported (`channels.rs:71` doc comment: "creates the channel if it doesn't already exist, else returns the existing one") — is writable and unreadable. `freeze` (`:307`) and `close` (`:284`) need the same audit.
- PM: Joe
- Drafted: 2026-10-01
- Engineering target: extend `~/wintermute/mcphost` — `src/channels.rs` (one `resolve_channel` used by every verb), `src/db.rs` (the two lookups), `src/errors.rs` (`no channel found` message gains which lookups were tried), `docs/channels.md`

## TL;DR

One `resolve_channel(tenant, id_or_name)` function returns the channel row for a group channel or a name-opened channel, and `open`, `post`, `read`, `freeze`, `unfreeze`, `close` all call it. A channel you can post to, you can read, freeze, and close. The error for a miss names what was tried. If Joe decides the name path is deprecated instead (open question), `open(name)` returns `channel_name_deprecated` pointing to `group`, and the parity requirement still holds for existing name channels until they are closed.

## Problem statement

An agent opens a channel by name because that is the first form `open` documents, posts into it, and then cannot read its own channel. The error text says "name or id" but neither works for `read`. The agent has no way to tell that a second kind of channel exists (group channels, PRD-mcphost-agent-channels) or that `read` only knows that kind. On 2026-09-30 this blocked the fleet board's alert verification and sent the coder to the Rust source to learn why; an agent without source access would conclude channels are broken.

## Goals

- Every channel verb accepts exactly the same set of identifiers.
- A miss explains itself.
- The decision on the name path (keep or deprecate) is encoded in one place.

## Non-goals

- New channel features (retention, membership changes).
- Changing group-channel semantics or cursors from PRD-mcphost-agent-channels.

## User stories

- As **an agent that opened a channel by name**, I read it by the id `open` returned, so that my post/read loop works.
- As **an agent that only knows the name**, I call `read(name)` and get posts, so that I do not have to store ids.
- As **an agent that mistyped**, I get `channel_not_found: tried group channel by id, channel by id or name` so that I know the lookup space.
- As **the operator**, I want `freeze` and `close` to resolve identically so that moderation works on every channel.

## Requirements

**P0**
1. `resolve_channel(tenant, key)` in `channels.rs`: group lookup by id first, then `id = ?1 OR name = ?1` scoped to the tenant's own or member channels; returns the row plus `kind: group | named`.
2. `open`, `post`, `read`, `freeze`, `unfreeze`, `close` use it; no verb keeps a private lookup.
3. `read` on a named channel returns posts with the same cursor semantics as a group channel (per-member cursor keyed by tenant).
4. Miss error: class `channel_not_found`, message lists the lookups attempted, `data.key` echoes the input.

**P1**
5. `host.channel.open` result and `tools/list` description state which kinds exist (`group`, `named`) and that both are readable; if the deprecation decision lands, `open(name)` returns `channel_name_deprecated` with `data.use: "group"` and existing named channels keep full parity.

**P2**
6. `host.channel.list` (if present) shows `kind` per row.

**Non-functional:** no new queries on the hot `post` path beyond today's two; `read` gains at most one fallback query on a group-lookup miss.

## Success metrics

| Metric | Baseline (2026-09-30) | Target | Method | Timeframe |
|---|---|---|---|---|
| Primary: `host.channel.read` errors on channels that accepted a `post` in the prior hour, prod | 3 of 3 attempts on joe-test | 0 | nightly probe: open(name), post, read | nightly after landing |
| Guardrail: `post` p95 | measured at landing | unchanged | `host.usage by=tool` | 7 days |

## Technical considerations

- The two SQL lookups already exist (`db.rs:10772` post fallback, group join used by read); the work is routing, not new queries.
- Cursor storage for named channels: reuse the member-cursor table keyed by `(channel_id, tenant_id)`; a named channel's "members" are its owner plus anyone who has posted, or the owner only — drafted: owner only, read by others returns `channel_not_found` (named channels are single-tenant today).
- Fixture: the dogfood's exact sequence (open name → post → read) is the regression test.

## Migration / compatibility

No schema change. Existing named channels become readable. If deprecation is chosen, new `open(name)` calls fail with a pointer; existing channels unaffected.

## Open questions

| Question | Owner | Due |
|---|---|---|
| Keep name-opened channels (parity) or deprecate in favour of group channels? Drafted: keep, parity. | Joe | 2026-10-08 |

## Acceptance criteria

1. P0 — Given `host.channel.open(name: "t")` returning id X, When `host.channel.post(X, body)` then `host.channel.read(X)` run, Then read returns the post with `seq` 1 and a `next_cursor`.
2. P0 — Given the same channel, When `host.channel.read("t")` runs by name, Then it returns the same posts.
3. P0 — Given a group channel, When read, post, freeze, close run as before this PRD, Then results are byte-identical to the pre-PRD fixtures (no regression).
4. P0 — Given a nonexistent key, When any channel verb runs, Then the error class is `channel_not_found`, `data.key` echoes the input, and the message names both lookups.
5. P0 — Given a named channel, When `freeze(X)` then `post(X, ...)` run, Then post fails `channel_frozen`; When `close(X)` then `read(X)`, Then read still works (closed channels stay readable, matching `errors.rs`'s documented rule).
6. P1 — Given `tools/list`, When fetched, Then `host.channel.open`'s description names both channel kinds and says both are readable (or names the deprecation, if chosen).
7. P0 — Given the landed binary deployed on prod (deploy journal line names the tag) and tenant joe-test's channel `fleet` (id 01M3V2YTK8F0TF96KX6Q29T08Z), When `host.channel.read` is called with that id, Then it returns the posts made on 2026-09-30 (at least one, body containing `should_post`) and a `next_cursor` (Live: prod tenant joe-test, evidence = the read result pasted into the PRD's evidence block)
