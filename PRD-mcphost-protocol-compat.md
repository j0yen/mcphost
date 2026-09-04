# PRD — mcphost protocol compatibility: make `tools/list` readable by the client that matters

- Status: queued
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- build_priority: high
- build_version_bump: patch
- publish: j0yen/private
- Vision: visions/mcp-host.md
- Loop: mcphost-buildloop: wow_rate — clears the `bootstrap` failure family at the transport layer
- PM: Joe Yen
- Drafted: 2026-09-03
- Engineering target: /home/jsy/wintermute/mcphost (Rust, rmcp 3.2.0)

## TL;DR

mcphost tells every client it speaks MCP `2026-07-28`, and then cannot serve a single
request at that version. The Claude Agent SDK — the one client this product exists for —
believes the advertisement, sends `MCP-Protocol-Version: 2026-07-28` on its next request,
and gets refused by rmcp's own SEP-2243 validation before mcphost's handler ever runs.
Separately, the unauthenticated `tools/list` response omits `ttlMs` and `cacheScope`, the
two fields that version makes mandatory. The result is a live, healthy, deployed endpoint
that shows a connected server with zero tools. This PRD makes the advertised protocol
version the one that was actually negotiated, and makes every `tools/list` response carry
the cache fields regardless of who asked.

## Problem statement

An AI agent authorized to build a tool on mcphost cannot see that mcphost has any tools.
It connects, `initialize` succeeds, the server reports itself connected, and the tool list
is empty — so the agent has nothing to call and no way to discover `signup`. The five-minute
wow path the whole product is built around fails at step one, for every agent, on the
deployed endpoint, before any candidate feature matters
(`evidence/mcp-host/findings-2026-09-03-first-live-session.md`, operator-run, 2026-09-03).

The consequence is measured and total. The live host at `https://178-105-64-66.sslip.io/mcp`
reports `tenants_total: 0` after two days of deployment: not one agent session has ever
completed `signup`. Every `measure.json` the loop has produced was a fake-mode run against
synthorg's own in-process endpoint scoring 1.0, and has been discarded. Zero trustworthy
measurements exist. The `claude-vibeloop-measure.timer` is installed and disabled. The
loop's ranking rule — the candidate whose segment shows the largest measured lift — has no
input at all, and cannot acquire one until an agent session can read a tool list.

Two independent defects in this repository produce that outcome. Both were verified by
reading the code and the vendored rmcp 3.2.0 source, not inferred from the symptom.

**Defect A — `tools/list` omits the SEP-2549 cache fields for every caller except a
tenant.** `src/handler.rs:549-550` returns `ListToolsResult::with_all_items(...)` for the
`Auth::Anonymous | Auth::Invalid` and `Auth::Admin` branches. Only the `Auth::Tenant`
branch (`src/handler.rs:583-585`) chains `.with_ttl_ms(...).with_cache_scope(...)`.
`with_all_items` initialises both fields to `None`
(rmcp 3.2.0 `src/model.rs:1624-1631`) and both carry
`skip_serializing_if = "Option::is_none"` (`src/model.rs:1603-1613`), so the JSON sent to
an unauthenticated caller contains neither key. rmcp's own doc comment on those fields
says they are "Required by spec version 2026-07-28". The anonymous branch is the first
call every new agent makes. The CLI's verbatim rejection —
`Invalid result for tools/list: ttlMs expected number got undefined; cacheScope expected
'public'|'private'` — names exactly these two fields.

The defect survived because the tests never covered that branch:
`tests/ac18_tools_list_ttl.rs:12` and `:54` both call `signup` and construct
`McpClient::with_bearer`, so AC18 only ever exercised `Auth::Tenant`, and
`tests/ac01_unauthenticated_lists_signup.rs` asserts on the tool name and count but never
on `ttlMs` or `cacheScope`.

**Defect B — the server advertises a protocol version it does not negotiate and cannot
serve.** `src/http.rs:24` hardcodes `MCP_PROTOCOL_VERSION = "2026-07-28"`, and the
`protocol_version_and_log` middleware stamps it onto every response that lacks the header
(`src/http.rs:75-80`). rmcp never sets that response header itself — all eight uses of
`HEADER_MCP_PROTOCOL_VERSION` in
`rmcp-3.2.0/src/transport/streamable_http_server/tower.rs` read incoming request headers;
none writes a response. So mcphost's middleware is the sole source of the advertisement,
including on the `initialize` response. But rmcp 3.2.0 sets
`ProtocolVersion::LATEST = V_2025_11_25` (`src/model.rs:175`), which is what `initialize`
actually negotiates. The vision's Lineage section states that rmcp 3.2.0 "serves the
2026-07-28 specification statelessly by default"; that is the mistaken belief this constant
was written from, and it is wrong — 2026-07-28 is in `SUPPORTED` and is
`STANDARD_HEADERS`, but it is not `LATEST`.

A client that trusts the response header then sends `MCP-Protocol-Version: 2026-07-28` on
every subsequent request, and rmcp rejects it twice over, both gated on the request header
being `>= 2026-07-28`: `validate_standard_headers` (`tower.rs:682-690`) demands SEP-2243
`Mcp-Method` headers and returns `-32020`, and `validate_request_protocol_version_meta`
(`tower.rs:520-535`) demands `_meta.io.modelcontextprotocol/protocolVersion` and
`clientCapabilities` and returns `-32602`. Neither is a configuration mcphost opted into:
`stateless_protocol_metadata_required` defaults to `false` (`tower.rs:176`) and mcphost
never enables it. The refusals happen purely because the client was told the wrong version.

There is one discrepancy between the code and the recorded evidence, and it must be settled
by a test rather than argued. The findings note that the reference `mcp` Python client 2.1.1
saw a well-formed `ttlMs: 0, cacheScope: "private"` on an unauthenticated `tools/list`,
which the code above says is impossible. The likely reconciliation is that the Python client
deserialises into a model whose defaults are exactly `ttl_ms: 0, cache_scope: Private`
(rmcp's own mirror of that shape does the same at `src/model.rs:1223-1224`), so a
client-side default was printed where the wire had nothing — while the TypeScript CLI
validates the raw payload strictly and reports `undefined`. AC2 asserts on the serialized
JSON, so it settles the question either way.

## Goals

- A Claude Agent SDK / Claude Code session pointed at the deployed endpoint lists `signup`
  with a complete input schema, using only the headers it sends by default.
- Every `tools/list` response carries `ttlMs` and `cacheScope`, in every authentication
  state, with no exception a future branch can reintroduce silently.
- The `MCP-Protocol-Version` mcphost advertises is the version it negotiated, and stays
  correct across an rmcp upgrade without a human remembering to edit a literal.

## Non-goals

- **Actually supporting protocol 2026-07-28.** Serving SEP-2243 `Mcp-Method` /
  `Mcp-Param-*` headers and per-request `_meta` is a substantial piece of work with no
  demand behind it: the client that matters negotiates 2025-11-25 today. This PRD makes
  mcphost stop lying about the version; it does not add the version. Recorded as an open
  question.
- **The tenant-key hand-off.** After this PRD an agent can *see* `signup`; it still cannot
  carry the returned key into the same session to publish and call. That is
  `PRD-mcphost-session-key`, which depends on this one.
- **Any consumer-facing feature candidate.** The standing goal ranks candidates by measured
  per-segment lift and no measurement exists yet.
- **Changing synthorg.** `consume --preflight` asserts only that the
  `MCP-Protocol-Version` header is present, not its value (`src/synthorg/consume.py:1679-1683`),
  so this change needs no harness edit.

## User stories

1. As an **authorized coding agent** (rapid prototyper segment), I connect to the mcphost
   endpoint and see `signup` in my tool list with its parameters, so I can call it correctly
   on the first attempt instead of guessing arguments three times.
2. As an **authorized coding agent**, I am never told the server speaks a protocol version
   that then refuses my requests, so I do not have to diagnose a transport failure to use a
   product whose whole promise is that setup is free.
3. As a **tenant agent** with published tools, my `tools/list` still returns `ttlMs: 0` for
   60 seconds after I publish, so my client re-reads the list and my new tool is callable
   immediately — unchanged by this PRD.
4. As the **operator**, I run `mcphost-deploy probe` after redeploy and the endpoint reports
   the version it actually negotiates, so the probe is evidence rather than decoration.
5. As the **loop**, my `bootstrap` failure family stops absorbing every session, so
   `wow_rate` becomes a number about the product rather than a number about the transport.

## Requirements

**P0 — cache fields on every list**

1. Every `tools/list` response emits both `ttlMs` (a JSON number) and `cacheScope` (the
   string `private` or `public`), in all four authentication states: anonymous, invalid
   bearer, admin, tenant.
2. `cacheScope` is `private` in every state. The anonymous list is identical for all
   callers, but a shared cache that could serve it to an authenticated caller — or the
   reverse — is a tenancy hazard with no compensating benefit at this traffic volume.
3. Anonymous, invalid-bearer and admin lists use the steady TTL
   (`TOOLS_LIST_TTL_MS_STEADY`); their contents do not change on a tool publish.
4. The tenant branch's existing behaviour is unchanged: `ttlMs` is `0` for
   `TOOLS_LIST_TTL_GRACE_SECS` after `last_tool_change_unix`, else the steady TTL.
5. The two fields are set in one place that every branch passes through, so a future
   `Auth` variant cannot omit them. A branch-by-branch chain of `.with_ttl_ms()` calls
   satisfies the ACs today and reintroduces the defect tomorrow; the build must not take
   that shape.

**P0 — honest protocol advertisement**

6. The `MCP-Protocol-Version` value mcphost sets on responses is derived from
   `rmcp::model::ProtocolVersion::LATEST`, not from a string literal in this repository.
7. A test asserts that the advertised value equals `ProtocolVersion::LATEST.as_str()`, so
   an rmcp upgrade that moves `LATEST` either updates the advertisement or fails the build.
8. mcphost never advertises a version for which `validate_standard_headers` would then
   require SEP-2243 headers that mcphost does not document — concretely, the advertised
   value is `< ProtocolVersion::STANDARD_HEADERS` for as long as requirement 12 is unmet.

**P1**

9. The middleware's doc comment is corrected: the layer is attached to the whole router
   (`src/http.rs:117-125`), so `/healthz` and `/.well-known/mcp/{ns}/server.json` receive
   the header too, not only `/mcp` as the comment claims. Either scope the layer to `/mcp`
   or state the actual scope; do not leave the comment contradicting the code.
10. `get_info`'s instructions text (`src/handler.rs:532-535`) is not degraded by this
    change. It is rewritten by `PRD-mcphost-session-key`; leave it alone here.
11. The structured request log continues to emit one line per request with `method`, `path`,
    `mcp_name`, `duration_ms` and `status`.

**P2**

12. Support for protocol 2026-07-28 — accepting and emitting SEP-2243 `Mcp-Method` /
    `Mcp-Name` / `Mcp-Param-*` headers and per-request `_meta` — deferred until a client
    that negotiates it exists.
13. When a request arrives declaring `MCP-Protocol-Version: 2026-07-28`, the error the
    client receives names the versions mcphost supports rather than surfacing rmcp's
    `-32020 missing required Mcp-Method header`, which is unactionable for a client author.
    Deferred: it requires intercepting rmcp's validation, and after requirement 6 no client
    is told to send that version.

## Success metrics

| metric | kind | baseline | target | method | timeframe |
|---|---|---|---|---|---|
| Tools visible to a default-configured Claude Agent SDK session on first connect | primary | 0 | ≥ 1 (`signup`, with schema) | AC12 replay test, then a live session against the deployed endpoint | at build, then first redeploy |
| `tools/list` responses missing `ttlMs` or `cacheScope` | primary | 3 of 4 auth states | 0 of 4 | AC1–AC5 assert on serialized JSON | at build |
| `tenants_total` on `/healthz` at the deployed endpoint | secondary | 0 | ≥ 1 from an agent session (not a raw-protocol probe) | `curl https://<endpoint>/healthz` | after `PRD-mcphost-session-key` |
| Sessions in the `bootstrap` failure family | guardrail | 1 of 1 (100%) | < 100% | `measure.json` `failures` after the next truth-tier run | first run after both PRDs |
| Existing test suite | guardrail | 105 tests green | 105 + new, all green | `cargo test --release` on RedBaron | at build |

## Technical considerations

- Both defects are in `src/handler.rs` (`list_tools`) and `src/http.rs`
  (`protocol_version_and_log`, `MCP_PROTOCOL_VERSION`). No database, kind, sandbox or
  control-plane code is touched.
- `rmcp::model::ProtocolVersion` exposes `LATEST` (`= V_2025_11_25`), `STANDARD_HEADERS`
  (`= V_2026_07_28`), `SUPPORTED`, and `as_str()`. Requirement 6 needs `as_str()` on a
  `const`; if it is not `const`-callable, compute it once at router construction and store
  it in `AppState` or a `OnceLock` rather than re-deriving it per request.
- `HeaderValue::from_static` takes a `&'static str`; a value derived at runtime needs
  `HeaderValue::from_str` with the parse error handled, or a `OnceLock<HeaderValue>`.
- Requirement 5 argues for resolving `(ttl_ms, cache_scope)` before the `match auth`, or
  applying them to the `ListToolsResult` after it — one place, all branches.
- `tests/common/` already provides `TestServer`, `McpClient`, `McpClient::with_bearer` and
  `signup`. AC12 needs a client that does *not* use those conveniences: it must send a raw
  POST sequence so the exact headers are under the test's control.
- Cargo runs on RedBaron. Version bump is `patch` (0.4.0 → 0.4.1): no new capability, and
  the wire change is a repair toward spec compliance.

## Migration / compatibility

The advertised `MCP-Protocol-Version` changes from `2026-07-28` to `2025-11-25`. Nothing
consumes the value: synthorg's preflight checks only for presence
(`src/synthorg/consume.py:1679-1683`), and `mcphost-deploy probe` reads `/healthz`, not
this header. No client can regress, because no client currently gets past `tools/list`.
Adding `ttlMs`/`cacheScope` to responses that previously omitted them is additive and
cannot break a client that ignored them.

## Open questions

| question | owner | due |
|---|---|---|
| Does mcphost commit to protocol 2026-07-28, and on what trigger — a client that negotiates it, or a registry requirement? | Joe | when a second real client appears |
| Should the `MCP-Protocol-Version` layer be scoped to `/mcp` (requirement 9's first option) or left router-wide and documented? | Joe | at build |
| Should `cacheScope` for the anonymous list be `public` once traffic justifies edge caching? | Joe | when the endpoint sees more than one client |

## Acceptance criteria

1. P0 — Given a server with no tenants, When a client with no `Authorization` header calls `tools/list`, Then the serialized JSON result contains a numeric `ttlMs` and a `cacheScope` of `"private"`, alongside exactly one tool named `signup`.
2. P0 — Given the same unauthenticated call, When the raw HTTP response body is parsed as JSON without any client-side model defaults applied, Then both the `ttlMs` and `cacheScope` keys are present in the result object.
3. P0 — Given a client sending `Authorization: Bearer not-a-real-key`, When it calls `tools/list`, Then the result contains a numeric `ttlMs` and `cacheScope: "private"` and lists only `signup`.
4. P0 — Given a client authenticated with the admin key, When it calls `tools/list`, Then the result contains a numeric `ttlMs` and `cacheScope: "private"` and lists the `admin.*` tools.
5. P0 — Given a tenant that has published no tools, When it calls `tools/list`, Then the result contains a numeric `ttlMs` and `cacheScope: "private"` and lists the ten `host.*` tools.
6. P0 — Given a tenant that published a tool less than `TOOLS_LIST_TTL_GRACE_SECS` ago, When it calls `tools/list`, Then `ttlMs` is `0`, preserving the behaviour `tests/ac18_tools_list_ttl.rs` already asserts.
7. P0 — Given a tenant whose last tool change was longer ago than the grace window, When it calls `tools/list`, Then `ttlMs` equals `TOOLS_LIST_TTL_MS_STEADY`.
8. P0 — Given the running server, When any client completes `initialize`, Then the response carries `MCP-Protocol-Version: 2025-11-25`, matching the `protocolVersion` in the `initialize` result body.
9. P0 — Given the source tree, When a test compares the advertised header value against `rmcp::model::ProtocolVersion::LATEST.as_str()`, Then they are equal, and no string literal `"2026-07-28"` remains as the advertised version in `src/http.rs`.
10. P0 — Given the advertised version, When a test compares it against `rmcp::model::ProtocolVersion::STANDARD_HEADERS.as_str()`, Then the advertised version is strictly lower, so SEP-2243 header validation is never triggered by mcphost's own advertisement.
11. P0 — Given a client that reads `MCP-Protocol-Version` from the `initialize` response and echoes it on the next request, When it calls `tools/list` with that header and with neither an `Mcp-Method` header nor a `_meta` object, Then it receives a successful result rather than JSON-RPC `-32020` or `-32602`.
12. P0 — Given a replay of the exact request sequence the Claude Agent SDK emits — `initialize` with `protocolVersion: "2025-11-25"`, then `notifications/initialized`, then `tools/list` carrying the `MCP-Protocol-Version` value taken from the `initialize` response header, no SEP-2243 headers, no `_meta` — When the sequence is run against a `TestServer`, Then `tools/list` returns HTTP 200 with a result whose single tool is `signup` and whose `inputSchema` declares the required property `name`.
13. P1 — Given a request to `POST /mcp` with `MCP-Protocol-Version: 2026-07-28` and no SEP-2243 headers, When rmcp refuses it, Then the server logs one structured request line recording the non-200 status, so the operator can see the refusal in `journalctl` without a packet capture.
14. P1 — Given `GET /healthz`, When it is called, Then the documented scope of the `MCP-Protocol-Version` layer in `src/http.rs` matches the observed behaviour — either the header is absent because the layer was scoped to `/mcp`, or the doc comment states that the layer is router-wide.
15. P0 — Given the full existing suite, When `cargo test --release` runs on RedBaron, Then all 105 pre-existing tests pass alongside the new ones, with no test deleted or weakened to accommodate this change.
