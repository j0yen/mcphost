# Metrics

Operator-facing numbers this host reports, and exactly how each is
computed. Read alongside `www/llms.txt`'s Operator notes; this file exists
for the metrics that need a longer definition than a one-line note.

## `oauth_tenants_7d`

PRD-mcphost-oauth-demand-signal requirement 4: the loop metric the two
OAuth PRDs (`mcphost-oauth-resource-server`,
`mcphost-hosted-authorization-server`) promise to move.

    oauth_tenants_7d = admin healthz's oauth.tenants_7d.issuer_jwt
                      + admin healthz's oauth.tenants_7d.hosted_token

Both addends are distinct **non-synthetic** tenants (the same `synthetic`
predicate `admin.tenants` uses) that made at least one call with that
credential kind in the trailing 7 days -- `calls.auth_method`
(`issuer_jwt` for a tenant-registered bring-your-own issuer bearer,
`hosted_token` for this host's own built-in authorization server) joined
against `tenants.synthetic IS NULL`. A tenant counted once per method it
used, not once per call.

`calls.auth_method`'s full domain also carries `key` (a tenant key, as a
header or a `tenant_key` argument) and, since v0.61.0, `session` -- a call
that presented no credential at all and resolved through the binding its
own connection's `signup`/`host.redeem` created
(PRD-mcphost-session-bound-tenant-after-signup). Admin `/healthz`'s `oauth`
block reports those as `session_bound_calls_24h`, a 24-hour window rather
than this section's 7/30-day pair, so an operator can see whether agents are
actually using the session binding:

    session_bound_calls_24h = admin healthz's oauth.session_bound_calls_24h

Unlike `oauth_tenants_7d` above it counts calls, not tenants, and does not
exclude synthetic tenants.

Read it with one curl from orch:

    curl -s -H "Authorization: Bearer $(cat ~/.config/mcphost/admin-key)" \
      https://<host>/healthz | jq '.oauth.tenants_7d.issuer_jwt + .oauth.tenants_7d.hosted_token'

`admin.oauth.demand_stats` reports the same two counts (plus `calls_7d`/`calls_30d`
by method, `clients`, `grants_active`, first-ever-call timestamps, a
per-tenant breakdown, and the registration/consent/token funnel) for a
caller holding the admin key.

## `help_url_served{code}`

PRD-mcphost-first-hour-support-surface requirement 8: which first-hour
error codes developers actually hit, by page view of the generated
`/help/<code>` page their `help_url` pointed at.

    help_url_served{code} = admin healthz's help_url_served.<code>

A process-wide, in-memory counter (`help::record_help_served`/
`help::help_hits_snapshot`) -- it resets on restart and is not persisted,
same tradeoff `hooks::EventCounters` already makes for
`events_received_1h`/`events_rejected_1h`. Absent from the object entirely
until a code's page has been served at least once; present codes only grow.

Read it with the same curl `oauth_tenants_7d` above uses:

    curl -s -H "Authorization: Bearer $(cat ~/.config/mcphost/admin-key)" \
      https://<host>/healthz | jq '.help_url_served'

## `tool_alias_calls{tool}`

PRD-mcphost-tool-naming-convention-and-aliases requirement 5: how many
calls each deprecated alias is still drawing, versus its canonical
replacement -- the number the sunset decision (user story: "alias calls
per day trending to zero before the sunset") is made from.

    tool_alias_calls{tool} = admin healthz's tool_alias_calls.<canonical name>

A process-wide, in-memory `{canonical: {alias: n, canonical: n}}` map
(`tool_aliases::record_call`/`tool_aliases::alias_metrics_snapshot`) --
same reset-on-restart, not-persisted tradeoff as `help_url_served{code}`
above. `alias` is every call made under a deprecated name that resolves to
`<canonical name>` (`src/tool_aliases.rs`'s `TOOL_ALIASES` table, e.g.
`host.tool_share` -> `host.tool.share`); `canonical` is every call made
under `<canonical name>` directly. Absent from the object entirely until
a canonical name with at least one alias has seen its first call (alias
or canonical); present names only grow.

Read it with the same curl `oauth_tenants_7d` above uses:

    curl -s -H "Authorization: Bearer $(cat ~/.config/mcphost/admin-key)" \
      https://<host>/healthz | jq '.tool_alias_calls'
