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
