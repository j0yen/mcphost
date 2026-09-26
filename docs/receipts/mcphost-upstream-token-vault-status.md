# Upstream token vault status — AC9 evidence receipt

PRD-mcphost-upstream-token-vault-status, AC9 (P0, Live):

> Given prod after deploy with the operator tenant holding provider `slack`
> registered from `preset: "slack"` with placeholder client credentials and no
> handoff (operator-provisioned Given, done by hand from orch with the operator
> key on mcphost-1 `/etc/mcphost/operator-tenant.key`), When
> `host.vault.status {end_user: "vaultst-probe"}` is called with the operator
> key over `https://mcphost.dev/mcp` and `admin.vault.stats` with the admin key
> from orch `~/.config/mcphost/admin-key`, Then status lists `slack` with
> `connected: false` and stats lists the operator tenant with
> `slack {tokens: 0}` (Live; evidence: healthz version after deploy plus both
> transcripts saved under `docs/receipts/<slug>.md`, no key or secret text).

AC9 has two halves and this receipt keeps them visibly separate:

| half | what it proves | state |
|---|---|---|
| the mechanism | a real `mcphost` server built from this branch serves `host.vault.status` / `admin.vault.stats`, and a registered-never-connected provider reads back `connected: false` / `tokens: 0` | **proven here**, by the transcripts below |
| the prod leg | that `mcphost.dev` *as deployed* serves them, and that the operator tenant's placeholder `slack` row exists on mcphost-1 | **PENDING the operator's post-ship run** — deferred, operator-provisioned |

No key, bearer token, client id or client secret appears anywhere in this
file; the transcripts below are response bodies only, and the vault's status
and stats surfaces never emit a token substring by construction (AC1/AC4).

## Branch-local transcript (every `cargo test`)

Captured by `tests/vaultst_ac09_live_vault_status_trailer.rs` against a real
server process on a real loopback socket — the same `mcphost::http` app
`main.rs` serves in production — with a fresh tenant standing in for the
operator tenant: `slack` registered through `preset: "slack"` with placeholder
client credentials, no handoff ever completed. That is AC9's Given exactly
except for *which* tenant, and *which* host.

This block is not transcribed by hand. `receipt_records_the_transcripts_the_server_actually_serves`
re-renders it from live response bodies on every `cargo test` run and fails if
it differs from what is committed here, so the receipt cannot drift away from
the code it vouches for. Two values are substituted, because they vary per run
rather than per behaviour: the stand-in tenant's generated namespace
(`<operator-tenant>`; on prod this is the operator tenant's own namespace) and
the crate version (`<crate version>`, asserted in that same test to equal this
build's `CARGO_PKG_VERSION`).

`GET /healthz` (admin bearer):

```json
{
  "db_ok": true,
  "version": "<crate version>"
}
```

`host.vault.status {"end_user": "vaultst-probe"}` (operator key):

```json
{
  "providers": [
    {
      "connected": false,
      "connected_at": null,
      "expires_at": null,
      "last_refreshed_at": null,
      "name": "slack",
      "revoked_at": null,
      "revoked_reason": null,
      "scopes": null
    }
  ]
}
```

`admin.vault.stats {}` (admin key):

```json
{
  "tenants": [
    {
      "providers": [
        {
          "name": "slack",
          "refresh_failures_24h": 0,
          "revoked": 0,
          "tokens": 0
        }
      ],
      "tenant_id": "<operator-tenant>"
    }
  ],
  "totals": {
    "refresh_failures_24h": 0,
    "revoked": 0,
    "tokens": 0
  }
}
```

The `slack {tokens: 0}` row above is the load-bearing one, and it is the
finding this receipt exists to record: `Db::vault_stats` originally aggregated
`FROM vault_tokens`, so a provider a tenant had registered but that no end
user had ever connected had no row to aggregate and was dropped from the
output entirely — indistinguishable, to the operator reading stats, from
"never registered". That is precisely AC9's state (placeholder credentials, no
handoff), so AC9's own Then could not have been satisfied on prod either. The
query now runs `FROM vault_providers ... LEFT JOIN vault_tokens`
(`src/db.rs`), and the zero row above is that fix, read back over HTTP.

## Prod leg — PENDING

Not run from the build sandbox, and not runnable from it: registering the
operator tenant's placeholder `slack` provider on mcphost-1 is an operator
action, and the operator key (`/etc/mcphost/operator-tenant.key` on mcphost-1)
and admin key (`~/.config/mcphost/admin-key` on orch) are operator-held. This
PRD therefore records AC9 under `deferred_acs` with reason
operator-provisioned, with the justification in its frontmatter
(`mock_justifications`) and the agreement of `agent/test-map.json` /
`agent/intent-card.json` locked by
`tests/vaultst_ac09_deferral_is_justified.rs`.

The check itself is written and waiting. After this branch ships and the
operator has hand-registered that provider, from orch:

```sh
# 1. the deployed version this evidence is pinned to
curl -sS -H "Authorization: Bearer $MCPHOST_ADMIN_KEY" \
  https://mcphost.dev/healthz | jq '{db_ok, version}'

# 2. both transcripts, through the same test that produced the block above
MCPHOST_LIVE=1 \
MCPHOST_URL=https://mcphost.dev \
MCPHOST_OPERATOR_KEY="$(cat /etc/mcphost/operator-tenant.key)" \
MCPHOST_ADMIN_KEY="$(cat ~/.config/mcphost/admin-key)" \
  cargo test --test suite_core_07 \
  vaultst_ac09_live_vault_status_trailer::operator_tenant_slack_shows_disconnected_and_zero_tokens \
  -- --nocapture
```

`MCPHOST_LIVE=1` redirects the identical `host.vault.status {end_user:
"vaultst-probe"}` (operator key, `https://mcphost.dev/mcp`) and
`admin.vault.stats` (admin key) calls at prod and prints both bodies in the
block shape above. Paste the printed version line and the two bodies into a
new "Prod leg — verified <date>" section here, replacing this one, and AC9 is
closed. Redact nothing by hand: neither body carries a credential.
