# PRD: mcphost-oauth-first-entry-url — an OAuth-only client pointed at mcphost never sees a door

- Status: building
- Lane: hand-ab (carbon op seat; A/B hand arm, Joe 10-13 11:1x pm "A/B"; claim = Status building + branch hand/mcphost-oauth-first-entry-url on j0yen/mcphost; plain PR, archive by hand)
- iter_log: 2026-10-14T06:05 claimed by the op seat for the hand-vs-daemon A/B (hand arm): sonnet coder on /mnt/data/hand/mcphost-oauth-first-entry-url, nice 19 / ionice idle / -j4; no daemon run; metrics on wiki/answers/2026-10-14-hand-vs-daemon-ab.md
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- Cited-tree: mcphost@686c77c0396e676e5c1611cd300cb92bfc8677e6
- build_priority: high
- test_prefix: oauthentry
- publish: j0yen/private
- Vision: visions/mcphost-one-url-onboarding.md
- Loop: mcphost-buildloop: oauth_tenants_7d
- Grounding: unauthenticated `initialize`, `tools/list`, and `tools/call` on `https://mcphost.dev/mcp` all returned 200 on 2026-10-13 3:1x pm PDT (implicit signup, `src/handler.rs:6503 implicit_signup_and_dispatch`); `oauth_401_upgrade` (`src/http.rs:1028`, codes at line 1060) rewrites only `tenant_key_missing|invalid_token|wrong_tenant`, which root no longer emits; PRD-mcphost-oauth-first-grant-signup's "new workspace" consent (landed PR #162, v0.103.2) is reachable only after a 401; synthorg `oauth_client.discover` (`oauth_client.py:170-193`) fails `expected 401, got 200` for 42/42 sessions on `explore-20261013T081050Z`
- PM: Joe
- Drafted: 2026-10-13
- Engineering target: j0yen/mcphost (`src/http.rs` router and middleware, `src/oauth.rs` metadata, `src/authz.rs` resource resolution, `www/` client pages, `llms-install.md`)

## TL;DR

`https://mcphost.dev/login/mcp` is the OAuth-first entry: every unauthenticated request on it, including `initialize`, answers 401 with `WWW-Authenticate: Bearer resource_metadata=".../login/mcp"`, its document lists mcphost's authorization server, consent offers key, claim code, or "new workspace", and bearer calls on it are served exactly as `/mcp`. The root stays the one URL for agents; the entry URL is the one URL for Claude.ai and ChatGPT, and the client pages name it. The path is one constant that the router, the metadata, the audience check, and the docs check read.

## Problem statement

A person whose only agent is Claude.ai or ChatGPT adds mcphost as a connector. Those clients run OAuth only when the server answers 401; mcphost's root answers 200 to everything unauthenticated since PRD-mcphost-implicit-signup (2026-10-01, Joe: one URL, never the key dance). The connector either connects as an anonymous implicit tenant the person can never log back into, or ChatGPT refuses it because its Apps directory requires OAuth (project memory: the ChatGPT Apps gating item is OAuth tenant login). PRD-mcphost-oauth-first-grant-signup built the consent branch for exactly this person and its AC 8 names a Claude.ai connector at `https://mcphost.dev/mcp`, but no path from that URL reaches consent: the live path is unexercised and therefore presumed broken. The same gap zeroes synthorg's OAuth read (42/42 explore sessions a night fail at `discover`).

Failure under this seed: yes — `curl` unauthenticated `initialize` on `/mcp` → 200 on 2026-10-13; explore OAuth cohort 0/42.

### Five whys

1. Why no OAuth? The client never receives a 401, and MCP has no other discovery trigger.
2. Why no 401? Root serves anonymous callers by design (implicit signup), and the 401 middleware keys off error codes root no longer produces (`src/http.rs:1060`).
3. Why did the OAuth PRDs ship against this root? PRD-mcphost-oauth-resource-server (401 on root) and PRD-mcphost-implicit-signup (200 on root) wrote opposite contracts for the same URL; implicit-signup superseded 20 tests of the other, including the 401 ones (its Technical notes, line 137), and nothing re-stated where OAuth clients should go.
4. Why did first-grant-signup's live AC pass? Its evidence was a deploy journal line plus a pasted whoami; no gate step started from the public URL with no credential and reached consent.
5. Why? The two personas (agent with a key-less SDK, human with an OAuth-only client) need opposite first responses, and the system had one URL and no persona signal. The root fix is a second, explicit entry whose contract is generated from one constant, not a per-client heuristic on `clientInfo.name` (that stays an open question).

## Goals / Non-goals

Goals: a tenant-less OAuth-first URL; 401 on every unauthenticated method there; tokens issued for it work there; docs name it for Claude.ai and ChatGPT; a conformance run from no tenant to `tools/list` through it.

Non-goals: changing root `/mcp`; per-client 401 heuristics at root; per-tenant URLs (PRD-mcphost-tenant-resource-metadata, shipped); OAuth metadata ordering (PRD-mcphost-oauth-metadata-lists-hosted-as).

## User stories

- Claude.ai user with no tenant: I paste `https://mcphost.dev/login/mcp`, Claude.ai opens the consent page, I choose "new workspace", and my tools appear.
- ChatGPT user: the Apps connector form accepts the URL because the server answers 401 with discovery metadata.
- Returning tenant owner: I log in with my key or claim code at the same consent page and land in my existing tenant.
- Operator: `admin.oauth.stats` shows registrations and calls that came through the entry URL.
- synthorg explore / journey-oauth: the OAuth door is one URL the chain env can name.

## Requirements

- P0 R1. One constant `OAUTH_ENTRY_PATH = "/login/mcp"` in `src/oauth.rs`; the router, the canonical-resource helpers, the metadata route `/.well-known/oauth-protected-resource/login/mcp`, and `www/sandbox-api-doc-check.sh`'s doc assertions all read it. A test asserts the docs and the router name the same path.
- P0 R2. Any request on the entry path without a valid bearer answers 401 with `WWW-Authenticate: Bearer resource_metadata="<public_url>/.well-known/oauth-protected-resource/login/mcp", scope="mcp"`, before JSON-RPC parsing, for every method including `initialize`.
- P0 R3. The entry document: `resource = <public_url>/login/mcp`, `authorization_servers = [<public_url>]`, `scopes_supported` as the root document.
- P0 R4. `/oauth/authorize` accepts `resource = <public_url>/login/mcp` with root semantics (`resource_tenant: None`: key, claim code, or "new workspace"); tokens carry that `aud`; bearer requests on the entry path are dispatched exactly as `/mcp` for the token's tenant, and a token minted for `/mcp` or a per-tenant resource is `wrong_audience` there.
- P0 R5. The conformance harness profiles `claude` and `chatgpt` run from no tenant against the entry URL through "new workspace" to `tools/list`.
- P1 R6. Client pages for Claude.ai and ChatGPT in `www/` and `llms-install.md` give the entry URL; `host.quickstart` returns `oauth_entry_url`.
- P1 R7. `admin.oauth.stats` adds `entry_url_registrations_7d` and `entry_url_calls_7d`.

## Success metrics

| metric | baseline | target | method | timeframe |
|---|---|---|---|---|
| OAuth-first clients that reach consent from the public URL with no credential | 0 (no path) | conformance 2/2 profiles | harness run in gate | at land |
| `oauth_tenants_7d` | 0 non-synthetic | ≥ 1 | `admin.oauth.stats` | 14 days after deploy |
| explore OAuth cohort reaching `token` | 0/42 | ≥ 40/42 | chain digest | first chain after synthorg points at the entry URL |

## Technical considerations

Middleware ordering: the entry-path 401 runs outside `oauth_401_upgrade` and before body parsing. Reuse `issue_tokens_for_subject`'s recorded-resource audience check (`src/authz.rs:1913-1921`) rather than a second allowlist. Streamable HTTP session handling on the entry path mirrors `/mcp`.

## Migration / compatibility

Additive route. Root behaviour unchanged. Existing tokens unaffected.

## Open questions

| question | owner | due |
|---|---|---|
| Should a known OAuth-only `clientInfo.name` at root also get the 401 (per-client heuristic)? | Joe | after 14 days of entry-URL stats |
| Claude.ai manual check from a fresh browser profile | Joe | after deploy |

## Acceptance criteria

1. P0 — Given no credential, When `POST /login/mcp` carries `initialize`, `tools/list`, or `tools/call`, Then each answers 401 with `WWW-Authenticate: Bearer resource_metadata="<public_url>/.well-known/oauth-protected-resource/login/mcp", scope="mcp"` and no tenant is created.
2. P0 — Given `GET /.well-known/oauth-protected-resource/login/mcp`, When fetched, Then `resource == "<public_url>/login/mcp"` and `authorization_servers == ["<public_url>"]`.
3. P0 — Given a DCR client, When it authorizes with `resource=<public_url>/login/mcp` and the consent form chooses "new workspace", Then a tenant with `source: "oauth"` exists, the token's `aud` is the entry resource, and `host.whoami` over `POST /login/mcp` with that bearer returns that tenant with `onboarding.url`.
4. P0 — Given a token minted for `<public_url>/mcp` or for `<public_url>/t/<ns>/mcp`, When presented on `/login/mcp`, Then 401 with error `wrong_audience`; Given the entry token presented on `/mcp`, Then `wrong_audience`.
5. P0 — Given the conformance harness profiles `claude` and `chatgpt` with no pre-existing tenant, When run against the entry URL, Then both reach `tools/list` through "new workspace".
6. P0 — Given the router, the metadata route, `www/` client pages, `llms-install.md`, and `host.quickstart`, When the `oauthentry` path-contract test runs, Then every occurrence of the entry path equals `OAUTH_ENTRY_PATH` and `sandbox-api-doc-check.sh` passes.
7. P0 — Given the landed binary deployed on prod, When an unauthenticated `initialize` is posted to the entry URL, Then the status is 401 and the challenge names the entry document (Live; evidence: `curl -s -o /dev/null -D - --max-time 15 https://mcphost.dev/login/mcp -H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream' -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"probe","version":"0"}}}' | grep -iE '^HTTP/|^www-authenticate'`)
