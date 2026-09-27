# Ship an MCP server to your own users

Your product already has a login (Auth0, Cognito, or your own OIDC
provider). Point mcphost at it once, and your users connect their own MCP
clients to *your* login -- no CIMD, no dynamic client registration, no
OAuth code in your product.

## Step 1: register mcphost as a client at your provider

Register one standard OIDC client (authorization code, PKCE) at your
identity provider and allow its callback:

```
host.oauth.provider_set(
  issuer="https://your-idp.example.com",
  client_id="<the client id your provider issued>",
  client_secret="<the client secret your provider issued>",
  scopes=["openid", "email", "profile"],
)
```

mcphost fetches your issuer's discovery document once (`.well-known/openid-configuration`,
https only) and stores `authorization_endpoint`, `token_endpoint`, and
`jwks_uri`. `client_secret` is encrypted at rest and never shown again;
`host.oauth.provider` shows everything else with no secret.

## Step 2: allow mcphost's callback URL at your provider

```
https://mcphost.dev/oauth/federation/callback
```

Add this exact URL to your provider's allowed redirect URIs. `host.oauth.doctor`
checks that your provider actually lists it (when your provider's own
discovery document exposes that).

## Step 3: give your users your per-tenant URL

```
https://mcphost.dev/t/<your-namespace>/mcp
```

`<your-namespace>` is the tenant namespace `host.whoami` reports. This is
the one URL your users' MCP clients ever see or paste in.

## Step 4: tell your users how to connect

- **claude.ai**: Settings -> Connectors -> Add custom connector -> paste
  the per-tenant URL from Step 3.
- **Claude Code**: `claude mcp add --transport http <name> https://mcphost.dev/t/<your-namespace>/mcp`
- **Any other OAuth MCP client**: point it at the per-tenant URL; it
  discovers the rest.

Connecting redirects the user's browser to *your* login (Step 1's
provider), then back to mcphost's own consent page naming the client and
the tools it wants, then back to the connecting client with a working
token. Your tools see the logged-in user as `MCPHOST_END_USER_ID` (their
provider subject, namespaced by your issuer so two providers can never
collide), `MCPHOST_END_USER_EMAIL`, and `MCPHOST_END_USER_NAME`; `host.enduser.whoami`
reports the same.

## Examples

### Auth0

```
host.oauth.provider_set(
  issuer="https://your-tenant.us.auth0.com/",
  client_id="<Auth0 application client ID>",
  client_secret="<Auth0 application client secret>",
)
```
In the Auth0 dashboard, add `https://mcphost.dev/oauth/federation/callback`
to the application's **Allowed Callback URLs**.

### Cognito

```
host.oauth.provider_set(
  issuer="https://cognito-idp.<region>.amazonaws.com/<user pool id>",
  client_id="<Cognito app client ID>",
  client_secret="<Cognito app client secret>",
)
```
In the Cognito app client's Hosted UI settings, add
`https://mcphost.dev/oauth/federation/callback` to the allowed callback
URLs.

### Generic OIDC

Any provider that publishes a standard `.well-known/openid-configuration`
discovery document works the same way: register a confidential
authorization-code client, allow the callback URL from Step 2, then
`host.oauth.provider_set` with that provider's own `issuer`/`client_id`/
`client_secret`.

## Managing federated logins

- `host.oauth.provider` shows your provider's endpoints and flags
  (`require_verified_email`, `owner_login`) -- never the secret.
- `host.oauth.doctor` diagnoses a misconfigured step: discovery
  reachable, `jwks_uri` reachable, this tenant's resource metadata,
  whether your provider's discovery lists the callback URL, and a
  dry-run authorize URL.
- `host.enduser.list`/`host.enduser.revoke {subject}` list and cut off
  individual federated users; `host.oauth.provider_remove` revokes every
  federated grant at once and falls back to key/claim (owner) login on
  your per-tenant resource.
- By default, only federated users can log in on your per-tenant
  resource -- `host.oauth.provider_set(owner_login: true)` also allows
  your own tenant key/claim code there, if you want to keep using the
  tenant owner login alongside your users' federated one.
