# mcphost

`mcphost serve` is a streamable-HTTP MCP server, stateless per the 2026-07-28
specification, on which an agent signs up with one unauthenticated tool call,
receives a tenant key, and then owns a namespace of tools it publishes, lists,
inspects and removes through further tool calls. There is no web page. The
operator administers tenants and reads metering through `admin.*` tools on
the same endpoint. Tool *execution* kinds (REST wrappers, code) are separate
PRDs; this one ships the endpoint, tenancy, the control plane, the `Kind`
trait, and a built-in `echo` kind so the harness can measure the bootstrap
path end to end.

Built from `PRD-mcphost-endpoint.md` (vision: `visions/mcp-host.md`).

## Install

```
cargo install --path .
```

Or build locally:

```
cargo build --release
./target/release/mcphost serve
```

### Environment contract

| Variable | Meaning | Default |
|---|---|---|
| `MCPHOST_DATA_DIR` | Directory holding `mcphost.db` (SQLite, WAL) | `./data` |
| `MCPHOST_BIND` | `host:port` to listen on | `127.0.0.1:8080` |
| `MCPHOST_PUBLIC_URL` | URL returned by `signup` as the endpoint | `http://<bind>` |
| `MCPHOST_ADMIN_KEY` | Bearer key that unlocks `admin.*` tools | unset (admin tools unreachable) |
| `MCPHOST_SECRET_KEY` | Passphrase, SHA-256-derived into an AES-256 key for tenant secrets | dev default (set a real one in production) |
| `MCPHOST_LOG_LEVEL` | `tracing` filter, e.g. `info` | `info` |

`mcphost migrate` applies pending SQL migrations and exits. `mcphost version`
prints the version and exits.

## Acceptance

Every P0 acceptance criterion is paired with a real `cargo test` (integration
tests under `tests/` spin up the server on an ephemeral port against a temp
`$MCPHOST_DATA_DIR`), except AC11 and AC12 which are hardware/external-tool
dependent and are recorded as smoke results below.

| AC | Requirement | Test |
|---|---|---|
| 1 (P0) | Unauthenticated `tools/list` shows only `signup`; response carries `MCP-Protocol-Version` | `tests/ac01_unauthenticated_lists_signup.rs` |
| 2 (P0) | `signup` returns key/namespace/endpoint; key stored only as a hash | `tests/ac02_signup_creates_hashed_tenant.rs` |
| 3 (P0) | Tenant `tools/list` shows `host.*` and no other tenant's tools | `tests/ac03_tenant_lists_control_plane_only.rs` |
| 4 (P0) | Publish, then list, then call round-trips | `tests/ac04_publish_list_and_call.rs` |
| 5 (P0) | Cross-tenant isolation: B can't see or call A's tool | `tests/ac05_cross_tenant_isolation.rs` |
| 6 (P0) | Remove a tool: omitted from list, `tool_not_found` on call | `tests/ac06_remove_tool.rs` |
| 7 (P0) | `host.usage`/`admin.usage` report calls + p50/p95 | `tests/ac07_usage_metering.rs` |
| 8 (P0) | `admin.tenant_disable` locks out a key; tenant key is `forbidden` on `admin.tenants` | `tests/ac08_admin_disable_and_forbidden.rs` |
| 9 (P0) | 6th signup/hour/IP is `rate_limited`, no tenant created | `tests/ac09_signup_rate_limit.rs` |
| 10 (P0) | Unregistered kind / invalid name / oversized spec each fail distinctly, nothing written | `tests/ac10_publish_validation_errors.rs` |
| 11 (P0, non-functional) | 200 concurrent `echo` calls, p95 < 50ms, 0 errors, RSS < 100MiB | `tests/ac11_load_smoke.rs` (`#[ignore]`d — hardware-dependent; run with `cargo test --release --test ac11_load_smoke -- --ignored --nocapture`). Measured on the build box: **p95 = 34.20ms, 0 errors, RSS = 37.3MiB** |
| 12 (P0) | `synthorg consume --preflight <url>` exits 0 | Smoke-verified: `uv run synthorg consume --preflight http://127.0.0.1:<port>/mcp` against a live `mcphost serve` → `ok — protocol 2026-07-28, 1 tools listed`, exit 0 |
| 13 (P0) | Mismatched `Mcp-Name` header vs. body is recorded by body name and flagged | `tests/ac13_mcp_name_mismatch_metering.rs` |
| 14 (P0) | Unwritable database: `storage` error, `/healthz` `db_ok: false`, process stays up | `tests/ac14_storage_unwritable.rs` |
| 15 (P0) | A call that never completes times out at the deadline, future dropped | `tests/ac15_call_timeout.rs` |
| 16 (P0) | 2MiB request body rejected with HTTP 413 | `tests/ac16_request_body_too_large.rs` |
| 17 (P0) | `Kind` conformance suite passes `echo`, fails naming `describe` for a bad schema | `tests/kind_conformance.rs` (reusable checker at `mcphost::kinds::conformance`) |
| 18 (P1) | `tools/list` carries `ttlMs`/`cacheScope`, `ttlMs: 0` within 60s of a publish | `tests/ac18_tools_list_ttl.rs` |
| 19 (P1) | `host.registry_publish()` + `/.well-known/mcp/<ns>/server.json` | **Deferred** — feature-flagged registry publish depends on the PRD's own open question ("registry namespace verification method... before P1 item 15"); no `host.registry_publish` tool or well-known route shipped this iteration |

## Related fleet work

- [`mcp-core`](https://github.com/j0yen/mcp-core) — the reusable stdio
  JSON-RPC 2.0 MCP-server core (`Tool` trait + `serve_stdio`) other wintermute
  MCP servers build on. Not reused here: `mcphost` is a streamable-HTTP
  server (`rmcp`), not a stdio server, and its tool surface is dynamic
  (per-tenant, DB-backed) rather than the static `Tool` trait `mcp-core`
  wraps. Cited per the PRD's technical considerations as related, not shared,
  code.

## License

Dual-licensed under MIT OR Apache-2.0 — see `LICENSE-MIT` and
`LICENSE-APACHE`.
