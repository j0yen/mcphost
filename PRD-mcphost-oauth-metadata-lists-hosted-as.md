# PRD: mcphost-oauth-metadata-lists-hosted-as — a tenant's OAuth metadata hides the only authorization server that can issue for it

- Status: building
- Lane: hand-ab (carbon op seat; A/B hand arm, Joe 10-13 11:1x pm "A/B"; claim = Status building + branch hand/mcphost-oauth-metadata-lists-hosted-as on j0yen/mcphost; plain PR, archive by hand)
- iter_log: 2026-10-14T06:05 claimed by the op seat for the hand-vs-daemon A/B (hand arm): sonnet coder on /mnt/data/hand/mcphost-oauth-metadata-lists-hosted-as, nice 19 / ionice idle / -j4; no daemon run; metrics on wiki/answers/2026-10-14-hand-vs-daemon-ab.md
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- Cited-tree: mcphost@686c77c0396e676e5c1611cd300cb92bfc8677e6
- build_priority: high
- test_prefix: oauthmeta
- publish: j0yen/private
- Vision: visions/mcphost-one-url-onboarding.md
- Loop: mcphost-buildloop: oauth_tenants_7d
- Grounding: `protected_resource_metadata_for_tenant` (`src/oauth.rs:988-1027`) lists the host's own issuer only when the tenant has an OIDC federation provider (`find_oauth_provider_by_tenant`, line 991), yet the hosted AS consents for any per-tenant resource (`resolve_resource_tenant`, `src/authz.rs:872-879`) and mints `aud` as that resource (`src/authz.rs:1913-1921`); prod `GET /.well-known/oauth-protected-resource/t/t_3556946d/mcp` returned `"authorization_servers":[]` on 2026-10-13 3:2x pm PDT while the root document lists `["https://mcphost.dev","toolconf-probe"]`; `trusted_issuer_set` (`src/oauth.rs:797`) stores any string as an issuer
- PM: Joe
- Drafted: 2026-10-13
- Engineering target: j0yen/mcphost (`src/oauth.rs` metadata builders and issuer setters, `src/authz.rs` audience, `src/http.rs` routes)

## TL;DR

Every tenant's RFC 9728 document lists mcphost's own authorization server first, because that server already issues tokens for every tenant. Issuer setters accept only absolute URLs, and both metadata documents filter rows that are not, so a client that reads `authorization_servers[0]` always gets a URL it can fetch `/.well-known/oauth-authorization-server` from. One validator decides both the write and the read, and a test asserts the two cannot drift.

## Problem statement

A tenant that already exists (an ICP row 1 agent owner, or synthorg's explore cohort standing in for one) points an OAuth client at its canonical resource `https://mcphost.dev/t/<ns>/mcp`. The client gets the correct 401 and fetches the tenant document, which says no authorization server exists. The client stops. synthorg's explore OAuth cohort fails 42 of 42 sessions a night this way (`explore-20261013T081050Z`, `sessions/admin_agent-11/session.json`: `oauth_trace = [{"step":"discover","status":"error"}]`), so the nightly read on OAuth clients has been zero information since the cohort shipped. The hosted authorization server would have issued a token for that very tenant: `resolve_resource_tenant` resolves the per-tenant resource to the tenant (`src/authz.rs:872-879`) and the token's `aud` is checked against the grant's recorded resource (`src/authz.rs:1920`). The document is wrong, not the server.

The second defect compounds it: `host.oauth.trusted_issuer_set` (`src/oauth.rs:797`) stores any string, so synthorg's toolconf probe left the literal `toolconf-probe` in prod's root document. RFC 9728 clients read `authorization_servers[0]` (synthorg `oauth_client.py:215`); today the URL sorts first only because `h` < `t`.

Failure under this seed: yes — 42/42 explore OAuth sessions fail at `discover` on 2026-10-13; the tenant document on prod lists no issuer for a tenant the hosted AS serves.

### Five whys

1. Why does the explore OAuth cohort fail? The client reaches no authorization server (root never 401s, see PRD-mcphost-oauth-first-entry-url; the tenant document lists none).
2. Why does the tenant document list none? `protected_resource_metadata_for_tenant` adds the host's own issuer only behind `find_oauth_provider_by_tenant` (`src/oauth.rs:991`).
3. Why that condition? PRD-mcphost-federated-end-user-login added the hosted issuer for its own case and preserved "exactly today's behaviour" for every other tenant (comment at `src/oauth.rs:985-987`), before any client walked the per-tenant document end to end.
4. Why did no test catch it? No test walks RFC 9728 from a published document to a token: tests assert document fields and token issuance separately.
5. Why? The contract between "which servers issue for this resource" and "which servers the document lists" is prose in two PRDs, not one function both sides read. Generate the contract: one function answers both.

## Goals / Non-goals

Goals: every tenant document lists the hosted AS; issuer values are URLs; one validator for write and read; one contract-walk test from document to token.

Non-goals: changing root `/mcp` behaviour (PRD-mcphost-oauth-first-entry-url); federation (`provider_set`) semantics; deleting legacy issuer rows (filtering suffices; the RedBaron seat removes `toolconf-probe` by hand).

## User stories

- Agent owner (ICP 1): I point Claude.ai at my tenant URL; the consent page appears instead of a connector error.
- SaaS PM (ICP 2): my tenant document lists my identity provider and mcphost's, in a stable order my docs can quote.
- Operator: a probe that mis-sets an issuer cannot corrupt the public documents.
- synthorg explore (synthetic): an OAuth session against a fresh tenant completes discover → token.

## Requirements

- P0 R1. `protected_resource_metadata_for_tenant` lists `public_url` first for every tenant, then the tenant's own issuers, deduplicated. The federation branch becomes a no-op.
- P0 R2. One function `valid_issuer_url(&str) -> bool` (absolute `https://` URL, or `http://` on a loopback host for tests; parsed with the `url` crate) gates `trusted_issuer_set`, `issuer_set`, and `provider_set` with error code `issuer_url_invalid`, and filters rows in both metadata builders.
- P0 R3. A test module `oauthmeta` walks the published contract with no hard-coded URLs: fetch the tenant document, take `authorization_servers[0]`, fetch its `/.well-known/oauth-authorization-server`, register, authorize with `resource` = the document's `resource`, consent with the tenant key, exchange the token, call `tools/call` on the resource with the bearer.
- P1 R4. A startup `tracing::warn!` names each stored issuer the validator rejects (count and tenant id, never the value), once per process.
- P1 R5. `www/llms.txt` and the OAuth docs page state the order: hosted AS first, own issuers after.

## Success metrics

| metric | baseline | target | method | timeframe |
|---|---|---|---|---|
| explore OAuth sessions passing the `metadata` step | 0/42 per night | ≥ 40/42 | synthorg explore digest `oauth_trace` | first chain after deploy once synthorg targets the per-tenant resource |
| tenant documents with empty `authorization_servers` | every tenant without a provider | 0 | admin healthz probe over 20 tenants | at deploy |
| non-URL issuers visible in any document | 1 (`toolconf-probe`) | 0 | `curl` both documents | at deploy |

## Technical considerations

Reuse `canonical_resource_uri` and `resolve_resource_tenant`. Validation uses the `url` crate already in the tree. The contract-walk test reuses the hosted-AS test client from PRD-mcphost-hosted-authorization-server's suite.

## Migration / compatibility

Additive: documents gain one entry; invalid rows stay in the table but leave the documents. Clients that cached an empty list refetch on the next 401.

## Open questions

| question | owner | due |
|---|---|---|
| Delete the `toolconf-probe` row on prod by hand before or after deploy? | RedBaron seat | before deploy |

## Acceptance criteria

1. P0 — Given tenant T with no issuers and no provider, When `GET /.well-known/oauth-protected-resource/t/<ns>/mcp` is called, Then `authorization_servers == ["<public_url>"]` and `resource == "<public_url>/t/<ns>/mcp"`.
2. P0 — Given T registered issuer I with `host.oauth.issuer_set`, When the tenant document is fetched, Then `authorization_servers == ["<public_url>", I]`; Given I equals `public_url`, Then it appears once.
3. P0 — Given `host.oauth.trusted_issuer_set` with issuer `toolconf-probe`, `ftp://x`, or `https://`, When called, Then the error code is `issuer_url_invalid` and no row is written; Given `https://idp.example.com` or `http://127.0.0.1:8080`, Then the row is written.
4. P0 — Given a stored issuer row whose value is `toolconf-probe`, When the root and the tenant documents are fetched, Then neither lists it and the process log carries one `issuer_url_invalid` warning naming the tenant id and count.
5. P0 — Given a fresh key-signup tenant on a test server, When the `oauthmeta` contract walk runs from its tenant document with no hard-coded authorization-server URL, Then `discover`, `metadata`, `register`, `authorize`, `consent`, and `token` succeed and a bearer `tools/call` on the document's `resource` returns the tenant's `host.whoami`.
6. P0 — Given the landed binary deployed on prod, When the tenant document for the joe-test tenant is fetched, Then its first authorization server is `https://mcphost.dev` (Live; evidence: `curl -s https://mcphost.dev/.well-known/oauth-protected-resource/t/t_3556946d/mcp | python3 -c 'import json,sys;print(json.load(sys.stdin)["authorization_servers"][0])'`)
