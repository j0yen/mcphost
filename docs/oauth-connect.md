# Connect over OAuth, no pasted key

mcphost hosts its own OAuth 2.1 authorization server (PKCE, S256 only), so
an OAuth-only MCP client -- claude.ai custom connectors, ChatGPT connectors
-- can connect without ever seeing a bearer key: give them the OAuth-first
entry URL, `https://mcphost.dev/login/mcp`, which answers 401 with the
discovery metadata those clients need (the root `/mcp` serves anonymous
agents directly and never challenges). Claude Code and Cursor can use the same flow, or paste a key directly
(see `docs/agent-quickstart.md`).

## claude.ai custom connector

1. In claude.ai, open **Settings -> Connectors -> Add custom connector**.
2. Enter `https://mcphost.dev/login/mcp` as the server URL.
3. claude.ai discovers `/.well-known/oauth-authorization-server`, registers
   itself dynamically (`POST /oauth/register`), and opens mcphost's
   `/oauth/authorize` consent page in a browser tab.
4. On the consent page, prove you own the tenant: either the tenant key
   (from `signup`) or a claim code from the claim email, then **Approve**.
5. claude.ai is redirected back with an authorization code, exchanges it
   at `/oauth/token`, and the connector is live -- no key ever pasted into
   claude.ai.

## Claude Code

```
claude mcp add --transport http mcphost https://mcphost.dev/mcp
```

Claude Code drives the same discovery -> register -> authorize -> token
exchange automatically the first time it connects, opening the consent
page in a browser.

## Cursor

Add an MCP server pointing at `https://mcphost.dev/mcp` (Cursor Settings ->
MCP -> Add new MCP server, transport `http`). Cursor follows the same
OAuth discovery and redirects you to the same consent page.

## Which authorization server a tenant document lists

`GET <public-url>/.well-known/oauth-protected-resource/t/<your-namespace>/mcp`
lists mcphost's own authorization server (`<public-url>`) first for every
tenant, then the issuers you registered with `host.oauth.issuer_set`,
deduplicated. A client that reads `authorization_servers[0]` therefore always
reaches an authorization server that can issue a token for your tenant.
Issuer values must be absolute `https://` URLs: `host.oauth.issuer_set`,
`host.oauth.trusted_issuer_set` and `host.oauth.provider_set` refuse anything
else with the error code `issuer_url_invalid`, and a stored value that is not
a URL is left out of both metadata documents.

## What the consent page asks for

The consent page names the connecting client and the resource it is
requesting (`<public url>/mcp`), then asks you to prove you own the
tenant with exactly one of:

- **Tenant key** -- the bearer key from `signup` or `host.key_rotate`.
- **Claim code** -- the single-use code from the claim email, if you
  haven't kept the key handy.

Approving mints a short-lived authorization code redirected back to the
client; nothing else on the page can skip this proof step.

## Managing connected clients

- `host.oauth.grants` lists every client connected to your tenant this
  way (`client_name`, `method`, `created_at`, `last_used_at`).
- `host.oauth.grant_revoke {id}` revokes one; its access tokens stop
  working within 60 seconds.
