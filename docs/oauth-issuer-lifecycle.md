# OAuth issuer lifecycle

A tenant can register OAuth issuers (`host.oauth.issuer_set`,
`host.oauth.trusted_issuer_set`). The protected-resource documents every
OAuth client reads first are derived from those rows:

- `/.well-known/oauth-protected-resource` lists this host as the hosted
  authorization server plus every stored issuer whose tenant exists and is
  enabled.
- `/.well-known/oauth-protected-resource/t/{namespace}/mcp` lists the hosted
  authorization server plus that tenant's own issuers.

Both documents run each stored row through the same validator the write verbs
use (an absolute `https://` URL with a host, no credentials, query or
fragment). A row that fails stays in the table but is never listed; the
server logs the rejected row ids once per document build, never the value.

## Check

`host.doctor` returns `oauth_issuers: {rows, invalid: [{id, tenant_id,
reason}]}`. `reason` is `not_https_url` (the validator refuses the stored
value) or `tenant_missing` (the owning tenant no longer exists). A tenant key
sees its own rows; the admin key sees every row.

## Remove

- Tenant: `host.oauth.issuer_remove {issuer}` returns `{removed: true}` and
  writes an `issuer_removed` row to `host.oauth.audit`. An issuer the tenant
  does not own returns `issuer_not_found`.
- Operator: `admin.oauth.issuer_remove {tenant_id, issuer, reason}` does the
  same for any tenant; the audit row carries the reason.

Two concurrent removals of one issuer produce exactly one `removed: true`,
one `issuer_not_found`, and one audit row.
