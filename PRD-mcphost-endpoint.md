# PRD — mcphost-endpoint: one MCP endpoint an agent can join and administer without a human

- Status: queued
- build_target: rust-cli
- publish: j0yen/private
- Vision: visions/mcp-host.md
- Depends-on: PRD-mcphost-harness.md
- Loop: mcphost-buildloop: wow_rate (bootstrap family)
- PM: Joe
- Drafted: 2026-09-02
- Redrafted: 2026-09-02 (Rust; RedBaron is the Rust build machine)
- Engineering target: new crate `mcphost` at ~/wintermute/mcphost (lib + binary `mcphost`), published j0yen/mcphost

## TL;DR

`mcphost serve` is a streamable-HTTP MCP server, stateless per the 2026-07-28
specification, on which an agent signs up with one unauthenticated tool call,
receives a tenant key, and then owns a namespace of tools it publishes, lists,
inspects and removes through further tool calls. There is no web page. The operator
administers tenants and reads metering through `admin.*` tools on the same
endpoint. Tool *execution* kinds (REST wrappers, code) are separate PRDs; this one
ships the endpoint, tenancy, the control plane, the `Kind` trait, and a built-in
`echo` kind so the harness can measure the bootstrap path end to end.

## Problem statement

An AI agent that wants a tool of its own on a reliable endpoint has nowhere to go
that does not require a human first. Every host the panel and the research named
starts from a connected repository and a console: Mcpfy deploys "from push"
[synthorg:1d7a3eddd6a8]; Azure Functions added an MCP trigger, inside Azure's
portal and identity model [synthorg:22a7ee57ba8a]; Cloudflare, Railway, Render,
Vercel and Cloud Run all begin at a dashboard
([diyai.io](https://diyai.io/ai-tools/hosting/best-mcp-server-hosting/)). The
brief says the target persona is the agent, that "all registration,
administration, operation and management tools for the entire stack will need to
be provided in the MCP server", and that there is no UI. The consequence today is
that 86% of MCP servers run on a laptop and most remote endpoints are dead or
degraded ([apigene.ai](https://apigene.ai/blog/host-mcp-server), April 2026 figure,
unattributed). The panel's integration specialists set the bar as a live,
TLS-terminated endpoint with zero setup steps (synthorg:99d2747c7ed7); the
prototypers said the first call has to work inside five minutes
(synthorg:ff7269026d35). Both are stated preference; the harness in
PRD-mcphost-harness.md is how this PRD is judged.

## Goals

- Sign-up, key, first `tools/list` in one session with no out-of-band step.
- Tenancy: a tenant sees and calls only its own tools plus the control plane.
- Stateless request handling so a second instance behind a plain load balancer
  needs no shared session state (spec 2026-07-28).
- Metering by tenant and tool from the `Mcp-Name` header, exposed to the operator
  as tools.
- A host process that stays responsive under a hostile tenant: no interpreter
  lock, bounded memory per request, cancellation on timeout.

## Non-goals

- Executing REST wrappers or user code (PRD-mcphost-rest-tools, PRD-mcphost-code-tools).
- OAuth authorization server. Bearer tenant keys ship first; the spec's
  authorization flow (CIMD) is a P2 here and a candidate later PRD.
- A web UI, an admin dashboard, or email. Nothing in this PRD renders HTML.
- Multi-box clustering. One box; the stateless design is what keeps a second one
  possible.

## User stories

- **Agent (any segment):** call `signup` with a display name and receive a key and
  my namespace, then reconnect with the key and see the control-plane tools.
- **Agent (rapid prototyper):** publish an `echo` tool under my namespace and call
  it within a minute, so I know the path works before I bring real code.
- **Agent (integration specialist):** list my tools, read the last twenty log lines
  of one, remove it, and see it gone from `tools/list` on the next call.
- **Operator (Joe):** with the admin key, list tenants, see calls and errors per
  tenant and per tool for the last 24 hours, and disable a tenant.
- **Harness (synthorg consume):** run `--preflight` against the endpoint and get a
  clean pass, then drive sessions with keys obtained through `signup`.

## Requirements

**P0**

1. Transport: streamable HTTP at `POST /mcp` using `rmcp` 3.x
   (`transport-streamable-http-server`, `StreamableHttpService` mounted on an
   `axum` router; `with_json_response(true)`; legacy session mode off so every
   protocol version is served statelessly). Every response carries
   `MCP-Protocol-Version`. `Mcp-Method` and `Mcp-Name` request headers, when
   present, are read for routing and metering and never trusted over the JSON
   body for authorization.
2. Authentication: `Authorization: Bearer <key>`. Keys are random 32-byte
   URL-safe tokens, stored as SHA-256 hashes. Unauthenticated requests may call
   only `initialize`, `tools/list` (which then lists only `signup`) and
   `tools/call` of `signup`.
3. `signup(name: str) -> {tenant, key, namespace, endpoint}` creates a tenant with
   namespace `t_<8 hex>`; rate-limited to 5 per source IP per hour; the key is
   shown once. A tenant record has `created_at`, `disabled`, `display_name`.
4. Control-plane tools, visible to an authenticated tenant, all under the
   reserved prefix `host.`: `host.whoami`, `host.tool_publish(name, kind, spec)`,
   `host.tool_list()`, `host.tool_remove(name)`, `host.tool_logs(name, limit=20)`,
   `host.usage(window="24h")`, `host.secret_set(name, value)`,
   `host.secret_list()`. Published tools are listed to their tenant as
   `<namespace>.<name>`.
5. Tool kinds are a registry behind a `Kind` trait in the library crate:
   `fn validate(&self, spec: &Value) -> Result<(), KindError>`,
   `fn describe(&self, spec: &Value) -> ToolDescriptor` (name, description,
   input schema), `async fn call(&self, spec: &Value, args: Value, ctx: &CallCtx)
   -> Result<Value, KindError>`, with `CallCtx` carrying tenant id, secret
   resolver, deadline and a call log handle. One built-in kind, `echo`: `spec`
   is `{"schema": <JSON Schema>}` and a call returns its arguments. Publishing a
   kind that is not registered fails with a JSON-RPC error naming the registered
   kinds. A conformance test suite (`tests/kind_conformance.rs`) runs against any
   `Kind` and is what the two feature PRDs must pass.
6. Publish latency: a tool published with `host.tool_publish` appears in that
   tenant's `tools/list` on the next request (no cache) and is callable; p95 under
   200 ms on the reference box.
7. Storage: SQLite at `$MCPHOST_DATA_DIR/mcphost.db` (WAL mode, `rusqlite` with
   the `bundled` feature), tables `tenants`, `tools`, `secrets` (values encrypted
   AES-256-GCM with a key from `$MCPHOST_SECRET_KEY`), `calls` (one row per
   `tools/call`: tenant, tool, started, duration_ms, ok, error_class), `logs`.
   Numbered migrations applied at start.
8. Metering: every `tools/call` writes a `calls` row; `host.usage` returns calls,
   errors and p50/p95 duration for the tenant over the window; `admin.usage`
   returns the same per tenant and per tool.
9. Admin tools, visible only with `$MCPHOST_ADMIN_KEY`: `admin.tenants()`,
   `admin.tenant_disable(tenant)`, `admin.tenant_enable(tenant)`, `admin.usage(window)`,
   `admin.tool_list(tenant)`. A disabled tenant's key is rejected with a
   JSON-RPC error `tenant_disabled`.
10. `GET /healthz` returns `{"version", "db_ok", "tools_total", "tenants_total"}`
    without authentication; used by the deploy probe.
11. Limits: a tenant may hold 50 tools; a tool name matches
    `^[a-z][a-z0-9_]{1,40}$`; a spec is at most 64 KiB; a request body at most
    1 MiB; a call result at most 1 MiB; a call is cancelled after 30 s (the
    `Kind::call` future is dropped at the deadline). Each limit returns a distinct
    error code.
12. Observability: structured JSON logs on stdout via `tracing` (one line per
    request: tenant, method, `Mcp-Name`, duration, status). No secrets in logs.
13. CLI: `mcphost serve` (reads the env contract: `MCPHOST_DATA_DIR`,
    `MCPHOST_BIND`, `MCPHOST_PUBLIC_URL`, `MCPHOST_ADMIN_KEY`,
    `MCPHOST_SECRET_KEY`, `MCPHOST_LOG_LEVEL`), `mcphost migrate`, `mcphost
    version`. A single static binary; no runtime dependencies on the box beyond
    libc.

**P1**

14. `tools/list` responses carry `ttlMs` and `cacheScope` (spec 2026-07-28), with
    `ttlMs` 0 for the first 60 s after a publish or remove for that tenant.
15. `host.registry_publish()`: write and serve the tenant's `server.json` at
    `/.well-known/mcp/<namespace>/server.json` with a `remotes` entry of type
    `streamable-http`, and submit it to `https://registry.modelcontextprotocol.io`
    (API v0.1) under the host's domain-verified namespace. Behind a feature flag
    until the domain is verified (vision open question).

**P2**

16. OAuth authorization per the 2026-07-28 specification with Client ID Metadata
    Documents, alongside bearer keys (`rmcp` `auth` feature).

**Non-functional**

- Reference box: Hetzner CPX21 class. 200 concurrent `tools/call` of `echo` from
  distinct tenants: p95 under 50 ms, zero errors, resident memory under 100 MiB.
- Cold process start to first successful `initialize` under 200 ms.
- All state in `$MCPHOST_DATA_DIR`; the process is otherwise stateless.
- `cargo test --release` green; `cargo clippy -- -D warnings` clean; rustc 1.88
  (RedBaron's toolchain).

## Success metrics

| metric | baseline | target | method | timeframe |
|---|---|---|---|---|
| primary: harness `wow_rate` on the `echo` task, truth tier | none (no endpoint) | ≥ 0.8 overall | `synthorg consume --measure` against the deployed endpoint | first truth-tier run after deploy |
| secondary: bootstrap failures | n/a | `failures.bootstrap` = 0 in the proxy tier; < 5% truth tier | `measure.json` | every run |
| secondary: publish-to-listed latency | n/a | p95 < 200 ms | `calls`/`logs` timing in the integration test | at ship |
| guardrail: cross-tenant visibility | n/a | 0 tools of another tenant ever listed or callable | integration test with two tenants | every run |

## Technical considerations

- Crates: `rmcp` 3.2 (`server`, `transport-streamable-http-server`, `macros`),
  `axum` 0.8, `tokio`, `tower-http` (request body limit, timeout), `rusqlite`
  (bundled, WAL), `serde`/`serde_json`, `jsonschema` (argument validation),
  `aes-gcm` + `rand` (secrets, keys), `sha2`, `tracing` + `tracing-subscriber`
  (JSON), `clap` (CLI). Verified: rmcp 3.2.0 on crates.io dated 2026-08-31; its
  README states it serves the 2026-07-28 draft statelessly by default and exposes
  `StreamableHttpService` as a Tower service, `with_json_response(true)`, and
  `#[tool_router]` / `#[tool_handler]` macros.
- The `rmcp` `ServerHandler` is implemented once; `list_tools` and `call_tool`
  resolve the tenant from the bearer key carried in request extensions by an
  `axum` middleware on every request and build results from the `tools` table
  plus the control plane. No in-memory registry that could drift between
  instances.
- `Mcp-Name` metering: when the header is absent (older clients), the tool name is
  taken from the JSON body after parsing; the `calls` row records which source.
- The `Kind` trait and `CallCtx` live in the library crate (`mcphost::kinds`)
  with `echo` as the reference implementation; the feature PRDs extend this crate
  (`build_into: ~/wintermute/mcphost`) and register their kinds in the same
  registry.
- Related work in the fleet, not reused: `mcp-core` (stdio JSON-RPC core; cite in
  the README), `mcp-register` (client config), `ai-stack` (data-layer parts bin).
- SQLite access runs on a dedicated blocking thread (`tokio::task::spawn_blocking`
  or a connection actor) so a slow query never stalls the async runtime.

## Migration / compatibility

New crate. The harness's fake endpoint is not this server; the harness preflight
is the compatibility contract between them. Schema migrations via numbered SQL
files applied by `mcphost migrate` and at `serve` start.

## Open questions

| question | owner | due |
|---|---|---|
| Public domain and TLS name | Joe | before mcphost-deploy |
| Registry namespace verification method (DNS or HTTP) | Joe | before P1 item 15 |
| Signup abuse beyond IP rate limiting (invite codes?) | Joe | after the first month of public sign-ups |

## Acceptance criteria

1. P0 — Given a fresh server with an empty data directory, When an unauthenticated client sends `initialize` then `tools/list`, Then the response carries `MCP-Protocol-Version` and lists exactly one tool, `signup`.
2. P0 — Given an unauthenticated client, When it calls `signup` with a name, Then it receives a key, a namespace matching `t_[0-9a-f]{8}`, and the endpoint URL, and the `tenants` table has one row with the key stored only as a hash.
3. P0 — Given a tenant key, When the client sends `tools/list`, Then it lists the `host.*` control-plane tools and no other tenant's tools.
4. P0 — Given a tenant, When it calls `host.tool_publish("hello", "echo", {"schema": {...}})` and then `tools/list`, Then `t_xxxxxxxx.hello` is listed on that same connection's next request, and `tools/call` on it returns the arguments passed.
5. P0 — Given two tenants A and B, When A publishes `hello` and B sends `tools/list` and `tools/call` on `A.hello`, Then B does not see it listed and the call returns a JSON-RPC error, not a result.
6. P0 — Given a tenant with a published tool, When it calls `host.tool_remove("hello")`, Then the next `tools/list` omits it and a call to it returns error `tool_not_found`.
7. P0 — Given 20 `tools/call` requests to a tool, When the tenant calls `host.usage("24h")`, Then it reports 20 calls with p50 and p95 durations, and `admin.usage` with the admin key reports the same under that tenant and tool.
8. P0 — Given the admin key, When `admin.tenant_disable(t)` runs, Then the next request with t's key returns error `tenant_disabled`; Given a tenant key sent to `admin.tenants`, Then the call is refused with `forbidden`.
9. P0 — Given a source IP that has signed up 5 times within an hour, When it calls `signup` a sixth time, Then the call returns error `rate_limited` and no tenant is created.
10. P0 — Given a publish with an unregistered kind, an invalid name, or a spec over 64 KiB, When `host.tool_publish` runs, Then each returns its distinct error code and nothing is written.
11. P0 — Given 200 concurrent `tools/call` of `echo` from 200 tenants on the reference box, When measured, Then p95 is under 50 ms, every call succeeds, and resident memory stays under 100 MiB.
12. P0 — Given a running server, When `synthorg consume --preflight <url>` runs, Then it exits 0.
13. P0 — Given a request with a mismatched `Mcp-Name` header and body tool name, When metered, Then the `calls` row records the body's name and the log line flags the mismatch.
14. P0 — Given the database file is unwritable, When any `tools/call` arrives, Then the server answers a JSON-RPC internal error naming `storage`, `/healthz` reports `db_ok: false`, and the process stays up.
15. P0 — Given a `Kind::call` that never completes, When the 30 s deadline passes, Then the caller receives error `call_timeout`, the future is dropped, and the server's task count returns to baseline.
16. P0 — Given a request body of 2 MiB, When posted to `/mcp`, Then the server answers HTTP 413 without reading the whole body into memory.
17. P0 — Given the conformance suite, When run against the `echo` kind, Then it passes; Given a `Kind` whose `describe` returns an invalid schema, Then the suite fails naming `describe`.
18. P1 — Given a tenant that just published a tool, When it sends `tools/list` within 60 s, Then the result carries `ttlMs: 0`; after 60 s, `ttlMs` is at least 30000.
19. P1 — Given the registry feature flag is on and the domain namespace is verified, When `host.registry_publish()` runs, Then `/.well-known/mcp/<namespace>/server.json` serves a valid document with a `streamable-http` remote and the registry API accepts it (mocked in tests).
