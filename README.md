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

> **For agents evaluating this host:** the machine-readable summary lives at [`/llms.txt`](https://mcphost.dev/llms.txt) on the production endpoint. Signup is one unauthenticated tool call; the quickstart there is six steps.

Built from `PRD-mcphost-endpoint.md` (vision: `visions/mcp-host.md`).

## Recent

- **v0.11.0** — `host.tool_publish` reports every simultaneously-invalid
  field at once (`data.errors`, each with its own `field`/`expected`/
  `example`) instead of one rejection per attempt; each kind's example
  spec/blurb and the new "Kinds" section below both render from
  `docs/kinds/*.md`, checked to match by
  `tests/publishfirsttry_ac06_docs_shared_source.rs`.
- **v0.4.0** — `args_schema` (and, for `python`, `requirements`) is now
  optional on the `python` and `http` kinds: when absent, the host derives it
  deterministically and offline from the source/templates the tenant already
  wrote (`src/kinds/infer.rs`). An explicit `args_schema` is used unchanged.
- **v0.1.2** — `synthorg consume --preflight` now has a real integration
  test (AC12); the `Kind` conformance suite moved to
  `tests/ac17_kind_conformance.rs`; `host.registry_publish` + `GET
  /.well-known/mcp/<namespace>/server.json` are implemented behind the
  `--registry-url` flag (AC19, see "Registry publish (P1)" below).

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
| `MCPHOST_REGISTRY_URL` | Enables `host.registry_publish` (P1) and names the registry API's base URL; `mcphost serve --registry-url <url>` takes precedence | unset (registry-publish disabled) |
| `MCPHOST_SIGNUP_RATE_LIMIT_PER_HOUR` | Overrides the per-source-IP `signup` rate limit (PRD-mcphost-signup-rate-configurable) — raise it for a many-session measure run from one IP; absent or non-integer falls back to the default. Effective value is logged once at startup | `5` |

`mcphost migrate` applies pending SQL migrations and exits. `mcphost version`
prints the version and exits. `mcphost serve --registry-url <url>` is the
CLI-flag form of `MCPHOST_REGISTRY_URL` above.

### Registry publish (P1)

Off by default. Once `--registry-url` / `$MCPHOST_REGISTRY_URL` names a
registry API base (e.g. `https://registry.modelcontextprotocol.io`):

1. The operator verifies a tenant's domain namespace by whatever method
   they trust (the PRD leaves the verification METHOD itself — DNS vs
   HTTP record — as an open question owned by Joe; this crate does not
   implement one) and records the outcome with `admin.tenant_verify_namespace`:
   `admin.tenant_verify_namespace(tenant="t_xxxxxxxx", domain_namespace="io.github.example.myserver")`.
2. That tenant can then call `host.registry_publish()` (no arguments): it
   POSTs a `server.json` document (`name`/`description`/`version`/`remotes:
   [{type: "streamable-http", url}]`) to `<registry-url>/v0/publish`, and
   the same document becomes servable, unauthenticated, at
   `GET /.well-known/mcp/<namespace>/server.json`.
3. `host.registry_publish` refuses with a distinct, machine-readable error
   in `data.error_code`: `registry_disabled` (flag off),
   `namespace_unverified` (step 1 not done for this tenant), or
   `registry_rejected` (the registry API answered non-2xx).

## Kinds

Every registered kind's minimal example spec, below, and `host.tool_publish`'s
on-wire description (visible from `tools/list` before signup) are both
rendered from the same `docs/kinds/*.md` files (PRD-mcphost-publish-first-try
requirement 6) -- `tests/publishfirsttry_ac06_docs_shared_source.rs`
regenerates this section from those files and fails CI if it's drifted from
what's checked in below. Call `host.quickstart(kind)` for the same example
with your own namespace already filled in.

<!-- kinds:start -->
### `echo`

spec.schema is any JSON Schema; a call echoes back the arguments it was given, validated against it.

Example spec:

```json
{
  "schema": {
    "properties": {
      "msg": {
        "type": "string"
      }
    },
    "required": [
      "msg"
    ],
    "type": "object"
  }
}
```

Example call arguments:

```json
{
  "msg": "hi"
}
```

### `http`

url must be an absolute https URL; method and url are the only required fields -- args_schema is inferred from the url/header/body templates when omitted.

Example spec:

```json
{
  "method": "GET",
  "url": "https://api.example.com/items/{{id}}"
}
```

Example call arguments:

```json
{
  "id": "123"
}
```

### `python`

only source is required -- args_schema and requirements are both inferred from it (tool-infer, v0.4.0); source must define main(args).

Example spec:

```json
{
  "source": "def main(args):\n    return {\"doubled\": args[\"n\"] * 2}\n"
}
```

Example call arguments:

```json
{
  "n": 3
}
```
<!-- kinds:end -->

## Acceptance

Every P0 acceptance criterion is paired with a real `cargo test` (integration
tests under `tests/` spin up the server on an ephemeral port against a temp
`$MCPHOST_DATA_DIR`), except AC11 which is hardware-dependent and is
recorded as a smoke result below.

### Sandbox suite: user namespace requirement

The `python` kind's sandbox suites (`tests/sandboxready_*`, `python_ac*`,
`infer_ac*`, `warmpool_ac*`, `ac17_kind_conformance`) spawn real `bwrap`/
`unshare` isolation and need unprivileged user namespaces
(`unshare --user --map-root-user -- true` must succeed) to run for real. If
your box denies that (Ubuntu's default AppArmor policy on some kernels, some
container runtimes), running `cargo test` fails loudly by design outside
CI, naming the fix: `sysctl kernel.unprivileged_userns_clone=1` on older
kernels, or `sysctl kernel.apparmor_restrict_unprivileged_userns=0` on
Ubuntu 24.04+. See `sandbox::require_user_namespaces_or_ci_skip`'s doc
comment for the full contract, and `.github/workflows/ci.yml` for how the
hosted CI runner grants the same capability (PRD-mcphost-ci-sandbox-coverage)
instead of silently skipping.

CI runs these suites as their own `sandbox` job, in parallel with the `gate`
job that carries static analysis and everything else — once the suites stopped
skipping, a single `cargo test --workspace` step measured 313–336 s against a
300 s budget. Which targets go where is derived, not hand-listed:
`scripts/ci-test-partition.sh core|sandbox` classifies every `tests/*.rs` by
whether it touches the sandbox-execution surface, and `check` proves the split
is total and disjoint. Both jobs then fail on any capability-skip in their log,
so a target filed into the wrong half turns CI red rather than passing
vacuously.

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
| 12 (P0) | `synthorg consume --preflight <url>` exits 0 | `tests/ac12_preflight.rs` — an always-run in-process half exercises the same two requests `run_preflight` makes; a second half spawns the real `mcphost` binary and the real `synthorg` CLI when available (bare binary or `uv run --project`) and asserts exit 0 |
| 13 (P0) | Mismatched `Mcp-Name` header vs. body is recorded by body name and flagged | `tests/ac13_mcp_name_mismatch_metering.rs` |
| 14 (P0) | Unwritable database: `storage` error, `/healthz` `db_ok: false`, process stays up | `tests/ac14_storage_unwritable.rs` |
| 15 (P0) | A call that never completes times out at the deadline, future dropped | `tests/ac15_call_timeout.rs` |
| 16 (P0) | 2MiB request body rejected with HTTP 413 | `tests/ac16_request_body_too_large.rs` |
| 17 (P0) | `Kind` conformance suite passes `echo`, fails naming `describe` for a bad schema | `tests/ac17_kind_conformance.rs` (reusable checker at `mcphost::kinds::conformance`) |
| 18 (P1) | `tools/list` carries `ttlMs`/`cacheScope`, `ttlMs: 0` within 60s of a publish | `tests/ac18_tools_list_ttl.rs` |
| 19 (P1) | `host.registry_publish()` + `/.well-known/mcp/<ns>/server.json` | `tests/ac19_registry_publish.rs` (mocks the registry API with `wiremock`; see "Registry publish (P1)" above — the namespace-verification METHOD stays out of scope, "verified" is an admin-set boolean) |

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
