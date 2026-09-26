# PRD: mcphost-upstream-token-vault-status — nobody can see whether a vault token is connected

- Status: queued
- Lane: orch 2026-09-26T14:41:23.694968108+00:00 run=263
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- Cited-tree: mcphost@8a0dadb9296f8dc1f94e89b24bb5222b0df4fe3c
- test_prefix: vaultst
- publish: j0yen/private
- Vision: visions/mcphost-end-user-auth.md
- Loop: mcphost-buildloop: satisfaction[workflow_orchestrator]
- Grounding: upstream-token-vault landed in 0.60.19 (run 257) with four of its own P0 requirements cut by the coder as "no non-deferred AC" (`agent/intent-card.json`; `runs/257/live/ac12.md` §2): `host.vault.status`, `host.vault.provider_remove`, `admin.vault.stats`, provider presets. `src/handler.rs:3022-3027` dispatches only `provider_set`, `providers`, `connect_link`, `disconnect`; `www/llms.txt` has zero lines naming the vault. Live AC12 of that PRD failed for lack of the status tool it named, and prod has zero `vault_tokens` rows with no admin surface that would show one if it existed
- PM: Joe
- Drafted: 2026-09-26
- deferred_acs: [9]
- mock_justifications: AC9 -- prod-only proof ("Given prod after deploy with the operator tenant holding provider `slack` registered from `preset: "slack"` with placeholder client credentials and no handoff (operator-provisioned Given, done by hand from orch with the operator key on mcphost-1 `/etc/mcphost/operator-tenant.key`)..."). It can only be satisfied by a real deploy of this branch plus the operator hand-registering the operator tenant's placeholder-credential slack provider on mcphost-1 and the admin key from orch; this coding sandbox has neither the deploy nor those operator-held credentials, so no run here can execute the prod leg. The live check is written and ready for the operator's post-ship run, gated on MCPHOST_LIVE=1: tests/vaultst_ac09_live_vault_status_trailer.rs, which prints the fetched `host.vault.status` and `admin.vault.stats` bodies for the `docs/receipts/<slug>.md` transcript. The same test runs the identical `host.vault.status` / `admin.vault.stats` calls against a real local mcphost server (a fresh tenant standing in for the operator tenant, with a `slack` provider registered via preset and never connected) on every cargo test -- that proves the branch's mechanism, not AC9, and is not counted as AC9's proof.
- Engineering target: j0yen/mcphost (`src/vault.rs`, `src/handler.rs`, `src/db.rs` vault section ~5190, `www/llms.txt`)

## TL;DR

A tenant can ask, per end user and per provider, whether an upstream token is connected, when it expires, and why it was revoked; the operator can read vault usage across tenants with the admin key; a tenant can register Slack, GitHub or Google with a preset instead of copying OAuth URLs, and can remove a provider. Every state the vault can be in becomes readable through a tool call, so the next live verification of a real handoff has evidence it can cite.

## Problem statement

An agent operator (workflow_orchestrator persona) wires an http-kind tool to Slack through the vault; when a call returns `upstream_not_connected` they cannot tell whether the end user never connected, the token expired, or the refresh was rejected, because 0.60.19 exposes no per-user view of `vault_tokens` (`src/vault.rs:262 resolve_for_call` reads the row, nothing reports it). The host operator cannot count connected tokens or refresh failures on prod at all: the 44 `admin.*` tools carry no vault entry. Consequence: the feature shipped 2026-09-26 with its live acceptance unverifiable (`runs/257/live/ac12.md`: "the AC's own named evidence mechanism does not exist"), and any future Slack handoff Joe performs leaves no readable trace beyond the tool call succeeding or not.

## Goals

- Per-end-user, per-provider connection state readable by the tenant.
- Cross-tenant vault usage readable by the operator.
- Provider registration by preset; provider removal that revokes its tokens.
- Agent-facing docs for the vault in `www/llms.txt`.

## Non-goals

- New OAuth flows, scopes negotiation, or token introspection against the provider.
- Rotating or re-encrypting stored tokens.
- The real Slack handoff itself (operator step; see Open questions).

## User stories

- **Agent operator (tenant):** after a user reports "the Slack tool stopped working", calls `host.vault.status` for that user and sees `connected: false, revoked_reason: "refresh_401"`, sends a fresh connect link.
- **Agent operator (tenant):** registers GitHub with `preset: "github"` and only a client id and secret.
- **Agent operator (tenant):** removes the `slack` provider after rotating the app; every stored token for it is revoked in the same call.
- **Host operator (Joe):** reads `admin.vault.stats` from orch with the admin key and sees tokens per provider per tenant and refresh failures in the last 24 h.
- **Live verifier:** reads status and stats over curl on prod and cites the JSON as evidence.

## Requirements

**P0**
1. `host.vault.status {end_user}` returns, for every provider the tenant registered, `{name, connected, expires_at, scopes, connected_at, last_refreshed_at, revoked_at, revoked_reason}`; `end_user: "self"` resolves through the call's verified end-user identity exactly as `connect_link` does, and a literal subject string resolves that subject for the tenant's own agent (no end-user identity required). Output never contains any token substring.
2. `admin.vault.stats` (admin key only) returns `tenants: [{tenant_id, providers: [{name, tokens, revoked, refresh_failures_24h}]}]` plus totals; `refresh_failures_24h` counts `vault_tokens` rows revoked in the last 24 h with a `revoked_reason` beginning `refresh_`. No new table.
3. `host.vault.provider_remove {name}` deletes the `vault_providers` row and marks every `vault_tokens` row for that provider `revoked_reason = "provider_removed"` in one transaction.

**P1**
4. `host.vault.provider_set` accepts `preset: "slack" | "github" | "google"`, filling `auth_url`/`token_url` from a table in `src/vault.rs` (Slack `https://slack.com/oauth/v2/authorize` + `https://slack.com/api/oauth.v2.access`; GitHub `https://github.com/login/oauth/authorize` + `https://github.com/login/oauth/access_token`; Google `https://accounts.google.com/o/oauth2/v2/auth` + `https://oauth2.googleapis.com/token`); explicit URLs override the preset; an unknown preset is `invalid_params`.
5. `www/llms.txt` gains an "Upstream token vault" section (placed above `<!-- sharing:start -->`) naming the five `host.vault.*` tools, the `upstream:` field on http-kind tool specs, the `upstream_not_connected` error and its `connect_link`, and the presets; `scripts/gen-docs-sharing.sh --check` stays green.

Non-functional: `host.vault.status` answers from one indexed query (`idx_vault_tokens_lookup`) per provider, p95 under 20 ms on the gate box with 10,000 token rows.

## Success metrics

| metric | baseline | target | method | timeframe |
|---|---|---|---|---|
| vault state readable on prod | no tool (ac12.md §2) | status + stats return 200 with the operator tenant's provider | AC9 live | at land |
| upstream-token-vault AC12 re-verification has citable evidence | "no evidence mechanism" | status JSON with `connected: true` after Joe's handoff | rerun `wm-build live 257` after the operator step | when Joe does the step |
| guardrail: no token bytes in any tool output | 0 (AC4 of the vault PRD) | 0 | AC1/AC4 | at gate |

## Technical considerations

- `src/vault.rs` already owns row lookup (`resolve_for_call`, `disconnect`); status reuses the same `Db` accessors (`src/db.rs` vault section) with a new read-only query returning the row minus the encrypted columns.
- Admin dispatch follows the existing `admin.*` arm pattern in `src/handler.rs`; tenant tools follow the `host.vault.*` arms at `src/handler.rs:3022-3027`.
- `end_user` as a literal subject is a tenant-scoped read of the tenant's own rows; the tenant already sees these subjects through `host.runs.get` (runs-end-user-subject PRD), so no new exposure.

## Migration / compatibility

No schema change; `compat: previous`. Tool list grows by three names.

## Open questions

| question | owner | due |
|---|---|---|
| Real Slack handoff on prod (register a Slack test app on the operator tenant, complete one browser handoff), then rerun `wm-build live 257`; this PRD gives that run its evidence tool | Joe | when convenient; not raised again until this PRD is live |

## Acceptance criteria

1. P0 — Given u1 connected to provider `slack` through the test OAuth server and provider `github` registered but never connected, When u1 calls `host.vault.status {end_user: "self"}`, Then `providers` lists `slack` with `connected: true`, a numeric `expires_at`, the granted `scopes`, `revoked_at: null`, and `github` with `connected: false`, and no substring of any access or refresh token appears in the response.
2. P0 — Given the same rows, When the tenant's agent (no end-user identity on the call) calls `host.vault.status {end_user: "<u1 subject>"}`, Then the same view returns; When `end_user` is omitted or `"self"` without an identity, Then `invalid_params` and `upstream_not_connected` respectively, matching `connect_link`'s behaviour.
3. P0 — Given u1's slack token revoked by a 401 on refresh, When u1 calls `host.vault.status`, Then `slack` shows `connected: false`, a numeric `revoked_at`, and `revoked_reason` beginning `refresh_`.
4. P0 — Given tenant t1 with two slack users and one refresh failure in the last 24 h, and tenant t2 with one slack and one github user, When the admin key calls `admin.vault.stats`, Then `tenants` lists t1 `slack {tokens: 2, revoked: 1, refresh_failures_24h: 1}` and t2 `slack {tokens: 1}` and `github {tokens: 1}`, `totals.tokens == 4`, and no token, secret, or `client_secret` substring appears.
5. P0 — Given a tenant key, When it calls `admin.vault.stats`, Then the call is rejected the same way every other `admin.*` tool rejects a tenant key.
6. P0 — Given provider `slack` with three connected users, When the tenant calls `host.vault.provider_remove {name: "slack"}`, Then `host.vault.providers` no longer lists it, all three `vault_tokens` rows carry `revoked_reason = "provider_removed"`, and each user's `host.vault.status` shows `connected: false` with that reason; When the name is unknown, Then `not_found`.
7. P1 — Given `host.vault.provider_set {name: "gh", preset: "github", client_id, client_secret, scopes: ["repo"]}` with no URLs, When it runs, Then the stored row carries GitHub's authorize and access-token URLs; Given the same call with an explicit `auth_url`, Then the explicit value wins; Given `preset: "gitlab"`, Then `invalid_params`.
8. P1 — Given `www/llms.txt`, When read, Then an "Upstream token vault" section above `<!-- sharing:start -->` names `host.vault.provider_set` (with the three presets), `host.vault.connect_link`, `host.vault.status`, `host.vault.disconnect`, `host.vault.provider_remove`, the `upstream:` tool-spec field, and `upstream_not_connected`, and `scripts/gen-docs-sharing.sh --check` exits 0.
9. P0 — Given prod after deploy with the operator tenant holding provider `slack` registered from `preset: "slack"` with placeholder client credentials and no handoff (operator-provisioned Given, done by hand from orch with the operator key on mcphost-1 `/etc/mcphost/operator-tenant.key`), When `host.vault.status {end_user: "vaultst-probe"}` is called with the operator key over `https://mcphost.dev/mcp` and `admin.vault.stats` with the admin key from orch `~/.config/mcphost/admin-key`, Then status lists `slack` with `connected: false` and stats lists the operator tenant with `slack {tokens: 0}` (Live; evidence: healthz version after deploy plus both transcripts saved under docs/receipts/<slug>.md, no key or secret text).
