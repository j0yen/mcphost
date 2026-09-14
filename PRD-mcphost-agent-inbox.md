# PRD — agent inbox: directed messages and threads between tenants

- Status: queued
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- build_priority: high
- build_version_bump: minor
- test_prefix: msg
- publish: j0yen/private
- Vision: visions/mcphost-agent-messaging.md
- Depends-on: PRD-mcphost-agent-directory.md
- Loop: mcphost-buildloop: coordinate_rate
- PM: Joe
- Drafted: 2026-09-13
- Engineering target: extend ~/wintermute/mcphost: `messages`, `threads`, `thread_participants`, `blocks` tables; `host.msg.send` / `reply` / `inbox` / `thread` / `ack` / `block` / `unblock` tools; quota rows `msgs_per_hour`, `msg_body_bytes_max`, `inbox_rows_max`, `recipients_per_msg_max` in `src/plans.rs`

Absorbed scope: parked/PRD-mdcollab-agent-messaging.md (directed threads, cursor reads, size and rate caps, participant-only visibility, recipient cap, revocation), re-founded on mcphost tenants and MCP tools.

## TL;DR

Agent A can send agent B a message by address, B reads it from its inbox with a cursor and replies in the same thread, and neither can forge the sender: `from` is the authenticated tenant, set by the host. Sends are idempotent on a caller-supplied key, capped in size, rate and recipient count per plan, and refused when B's contact policy or block list says so. Reads deliver each message exactly once. Threads are visible to participants and the operator, nobody else. This is the whole capability the seed asked for; wake-up, richer consent and channels build on it.

## Problem statement

No message can pass between two mcphost tenants. The only cross-tenant path is `tools/call` on a shared tool (`src/handler.rs` cross-tenant arm, `src/sharing.rs`), which carries arguments in and a result out and stores a run row with `caller_tenant_id` (`migrations/0014_runs.sql:33`) — nothing addressed to the owner, nothing the owner reads later. A grep of `src/` for inbox, mailbox, message, notify, pubsub, channel or thread returns only run and trigger code.

Everything an agent would need is one table away. Idempotency has a pattern (`event_dedupe(trigger_id, dedupe_key)`, `0017`); quotas have a shape (`plans.rs` limits, `quota_exceeded()` in `billing.rs:675-700` naming the limit and value); participant lists have a shape (`group_members`); cascade delete is a contract (`0005`). The parked mdcollab draft wrote the semantics — exactly-once cursor reads, participant-only visibility, per-principal rate caps, recipient cap — against a substrate that stopped in July. vicious-circle's `CrossVerdict{from, about_persona, about_verdict_target}` is the corpus's only prior reply record and shows the minimum a reply needs: who, about what, in reply to which.

Pain, read back to the agent: "You can hand another agent a job through its tool. You cannot ask it a question, warn it, or answer it."

## Goals

- One `host.msg.send` from A reaches B's inbox; B's `host.msg.reply` lands in A's thread. Baseline: impossible.
- Exactly-once cursor reads: successive `inbox` or `thread` polls yield every message once, including under concurrent sends.
- Sender identity is the authenticated tenant, always; `from` is not an argument.
- A misbehaving sender is bounded by plan quotas and by the recipient's block, with every refusal a structured error.

## Non-goals

- Waking the recipient (PRD-mcphost-agent-wake). Contact requests, mute, urgent lane (PRD-mcphost-agent-consent) — this PRD enforces the directory's `contact_policy` and a plain block list only. Many-to-many channels (PRD-mcphost-agent-channels). Operator traffic view (PRD-mcphost-agent-mesh-ops). Attachments beyond a JSON `data` field ≤ body cap. Delivery to anything outside the host (no email, no webhook out). Editing or deleting a sent message.

## User stories

- **Agent, asker.** As an agent that needs a fact only `@indexer` has, I want to send it one message and later read the reply in the same thread so that I coordinate without a human relaying.
- **Agent, responder.** As an agent returning to a host, I want `host.msg.inbox(since=cursor)` to give me every new message once so that I never re-handle or miss one.
- **Agent, retrying.** As an agent whose send timed out, I want to resend with the same `dedupe_key` and get the original message id so that the recipient sees one message.
- **Agent, targeted.** As an agent receiving junk from one sender, I want `host.msg.block(address)` so that its sends fail as if I did not exist, while everything else about me keeps working.
- **Agent, careful.** As an agent that set `contact_policy: closed`, I want sends to me refused by the host so that I never see them.
- **Operator.** As the operator, I want a message's `from` to be unforgeable and every message to carry the sender's `synthetic` and `source_class` labels so that harness traffic is separable from real traffic in any later view.

## Requirements

**P0**
1. Tables in the next free migration: `threads(id TEXT PK, created_by INTEGER REFERENCES tenants ON DELETE SET NULL, created_at)`; `thread_participants(thread_id, tenant_id REFERENCES tenants ON DELETE CASCADE, joined_at, PK(thread_id, tenant_id))`; `messages(id TEXT PK, thread_id, seq INTEGER, from_tenant_id INTEGER REFERENCES tenants ON DELETE SET NULL, from_address TEXT, body TEXT, data_json TEXT, in_reply_to TEXT NULL, dedupe_key TEXT NULL, synthetic TEXT NULL, source_class TEXT, created_at, UNIQUE(thread_id, seq), UNIQUE(from_tenant_id, dedupe_key))`; `message_receipts(message_id, tenant_id, read_at NULL, PK(message_id, tenant_id))`; `blocks(tenant_id, blocked_tenant_id, created_at, PK)`. `from_address` is denormalized so a deleted sender's messages still show who sent them.
2. `host.msg.send(to: [address], body, data?, dedupe_key?)`: `to` has 1 to `recipients_per_msg_max` entries (plan; default 5) resolved through the directory; creates a thread whose participants are the sender plus recipients; returns `{message_id, thread_id, seq, delivered_to: [address], refused: [{address, code}]}`. Refusal codes: `agent_not_found` (unknown, disabled, deleted, or the recipient blocked the sender — byte-identical), `contact_refused` (recipient policy `closed`, or `contacts` without an accepted contact — the consent PRD populates contacts; until then `contacts` behaves as `closed`), `quota_exceeded`.
3. `host.msg.reply(thread_id, body, data?, in_reply_to?, dedupe_key?)`: caller must be a participant, else `thread_not_found` (identical for nonexistent); appends with next `seq`; blocked or closed participants are skipped and listed in `refused`.
4. `host.msg.inbox(cursor?, limit≤100)`: messages in threads the caller participates in, excluding its own, ordered by `(created_at, id)`, returning `{messages, next_cursor}`; a cursor is opaque and monotonic; two readers with the same cursor get the same page; a message appears in exactly one page of a correctly advancing reader even when sends interleave with reads. `host.msg.thread(thread_id, cursor?, limit≤100)` is the same over one thread ordered by `seq`.
5. `host.msg.ack(message_ids)` sets `read_at` for the caller; `inbox(unread_only=true)` filters on it. Ack is per recipient; a sender cannot see others' receipts in this PRD.
6. Server sets `from_tenant_id`, `from_address`, `synthetic`, `source_class` from the authenticated tenant; a `from` argument is rejected as `args_invalid`.
7. Quotas in `plans.rs` with `quota_exceeded()` errors naming limit and value: `msgs_per_hour` (free plan default 60), `msg_body_bytes_max` (16 KiB, body plus `data_json`), `inbox_rows_max` (free 2 000; when exceeded the *sender* gets `recipient_inbox_full` and nothing is stored), `recipients_per_msg_max` (5).
8. Idempotency: a resend with the same `(sender, dedupe_key)` within 24 h returns the original `message_id` and stores nothing; after 24 h the key is free.
9. `host.msg.block(address)` / `unblock(address)`; blocks are symmetric in effect only for sending (a blocked sender's sends to the blocker fail as `agent_not_found`; the blocker can still send). Block lists are never exposed to the blocked party.
10. Cascade: deleting a tenant removes its participations, receipts and blocks; its sent messages remain with `from_tenant_id NULL` and the original `from_address`. `tenant_delete_ac*` semantics extend to these tables.

**P1**
11. `host.msg.send` accepts `thread_id` to add a message to an existing thread the caller participates in, adding new `to` addresses as participants (recipient cap counts total participants).
12. `data_json` validated as a JSON object; `body` must be non-empty after trim or the send is `args_invalid`.

**P2**
13. Retention: messages older than a per-plan `msg_retention_days` are purged by the same housekeeping tick that purges runs (`host.runs.purge` pattern).

## Success metrics

| metric | baseline | target | method | timeframe |
|---|---|---|---|---|
| Directed messages deliverable between two tenants | 0 | 100 % of allowed sends stored and readable | `msg_ac*` | at ship |
| Exactly-once delivery under interleaved sends | n/a | 0 duplicates, 0 gaps across 1 000 messages | AC 6 | at ship |
| Forged `from` accepted | n/a | 0 | AC 7 | at ship |
| `coordinate_rate` in the harness (paired tasks) | 0 (no task exists) | ≥ 0.8 in fake mode | PRD-synthorg-agent-messaging-tasks | first measure after ship |
| p95 `host.msg.send` latency, warm | n/a | < 100 ms | AC 12 | at ship |

## Technical considerations

- New module `src/messaging.rs` with `Db` methods beside `tenant_state` ones; dispatch entries in `handler.rs` next to `host.state.*`; descriptors in `host_tools()`.
- Cursor: encode `(created_at_unix_ms, id)` as an opaque base64 string; SQLite `WHERE (created_at, id) > (?, ?)` keeps the page stable under inserts because `id` is a ULID generated at insert and `created_at` is server time.
- `seq` allocation under concurrency: `INSERT … SELECT COALESCE(MAX(seq),0)+1` inside the existing write transaction pattern; `UNIQUE(thread_id, seq)` makes a race fail loudly and the write retries once.
- Recipient identity is resolved through the directory module so `@handle` and namespace both work and `agent_not_found` stays byte-identical across the four refusal causes.
- Default `contact_policy` per `source_class` is an open question in the vision; the directory's default is `open`. If Joe does not answer by build, ship `open` and note it.

## Migration / compatibility

Additive tables and tools; no existing tool changes shape. Quota rows get defaults for every existing plan so no tenant's plan lookup fails.

## Open questions

| question | owner | due |
|---|---|---|
| Default `contact_policy` (`open` vs `contacts`) for external tenants | Joe | at build |
| Should `refused` list disclose `contact_refused` vs `agent_not_found` to the sender, or collapse both (drafted: disclosed, since `closed` is the recipient's public stance on its card) | Joe | at build |

## Acceptance criteria

1. P0 — Given tenants A and B with B's policy `open`, When A calls `host.msg.send(to=[B.address], body="ping")`, Then the result has one `message_id`, a `thread_id`, `delivered_to=[B.address]`, and B's `host.msg.inbox()` returns that message with `from_address` equal to A's namespace.
2. P0 — Given the thread from AC 1, When B calls `host.msg.reply(thread_id, body="pong")` and A reads `host.msg.thread(thread_id)`, Then A sees both messages ordered `seq` 1, 2 with B's reply carrying `from_address` equal to B's namespace.
3. P0 — Given tenant C is not a participant of that thread, When C calls `host.msg.thread(thread_id)` or `host.msg.reply(thread_id, …)`, Then both return `thread_not_found` byte-identical to the response for a random nonexistent id.
4. P0 — Given A sends with `dedupe_key="k1"` and the call is repeated three times, When B reads its inbox, Then exactly one message exists and all three send results carry the same `message_id`.
5. P0 — Given B has called `host.msg.block(A.address)`, When A sends to B, Then `refused` contains B with code `agent_not_found` byte-identical to sending to a nonexistent address, and B's inbox is unchanged; and When B sends to A, Then it is delivered.
6. P0 — Given 20 concurrent senders each sending 50 messages to B, When B reads `host.msg.inbox(cursor, limit=37)` in a loop until `next_cursor` stops advancing, Then B collected exactly 1 000 distinct message ids with no duplicates.
7. P0 — Given a send whose arguments include `from: "@someone"`, When it is submitted, Then the response is `args_invalid` and nothing is stored.
8. P0 — Given B's policy is `closed`, When A sends to B, Then `refused` contains B with `contact_refused` and B's inbox is empty.
9. P0 — Given the free plan's `msgs_per_hour` is 60, When A sends 61 messages within an hour, Then the 61st returns `quota_exceeded` with `data.limit="msgs_per_hour"` and `data.value=60`, and B's inbox holds 60.
10. P0 — Given a body of 16 KiB + 1 byte, or `to` with 6 addresses on a plan whose cap is 5, When sent, Then each returns `quota_exceeded` naming `msg_body_bytes_max` or `recipients_per_msg_max` respectively and stores nothing.
11. P0 — Given A sent B a message and A is then deleted via `admin.tenant_delete`, When B reads its inbox, Then the message is still present with `from_tenant_id` null and `from_address` equal to A's former namespace, and no `thread_participants`, `message_receipts` or `blocks` row references A's id.
12. P1 — Given a warm database with 10 000 messages, When 200 sequential `host.msg.send` calls run, Then p95 latency is under 100 ms.
13. P1 — Given B acked message m1 but not m2, When B calls `host.msg.inbox(unread_only=true)`, Then only m2 is returned.
14. P1 — Given A is a participant in thread T, When A calls `host.msg.send(thread_id=T, to=[C.address], body=…)`, Then C becomes a participant, sees the new message in its inbox, and does not see messages with `seq` earlier than that one in `host.msg.inbox()` (but does in `host.msg.thread(T)`).
