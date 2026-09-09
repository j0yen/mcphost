# mcphost

<!-- agent-quickstart:start -->
Ship an MCP tool, not a deployment project.

mcphost lets an agent create the tool it needs, mid-task, without a human
in the loop: sign up with one unauthenticated tool call, publish with the
next, and the new tool is live immediately — no restart, no deploy, no
review queue.

**Measured** (panel run `0.26.3-20260908T085001Z`, 21 sessions): median
time from signup to a tenant's first successful `host.tool_publish` is
**30.7s**; median time from signup to a successful call on that tenant's
own tool is **42.4s**.
<!-- cite: docs/benchmarks/measure-0.26.3-20260908T085001Z.md -->

## Quickstart for agents

1. Connect to the endpoint and call `tools/list` with no credentials. The
   only tool offered is `signup`.
2. Call `signup(name)`. The response contains `tenant`, `key` (a bearer
   token, shown once), `namespace`, and `endpoint`. Signup is rate-limited
   to 5 per IP per hour.
3. Reconnect with `Authorization: Bearer <key>`. The `host.*` control
   plane is now available.
4. Publish a tool: `host.tool_publish(name, kind, spec)`. Three ways: wrap
   an API you already use (`http` — url and method required, `args_schema`
   inferred if omitted), submit code (`python` — source required,
   `args_schema`/`requirements` inferred if omitted), or test the pipes
   (`echo` — returns its arguments; spec is a JSON Schema). Dry-run first
   with `host.spec_test(kind, spec, invocations)` — up to 5 example calls
   through the same sandbox a real call uses, no tool row written until
   you're green.
5. Call your tool. Two equivalent ways over the same streamable-HTTP
   connection: as `<namespace>.<tool_name>` (its own entry in
   `tools/list`), or `host.tool_call(name, args)` (same dispatch path,
   useful when your client doesn't refresh `tools/list` between publish
   and call). `host.tool_test(name, args)` dry-runs an already-published
   tool by name instead of a raw spec.
6. Check the plan and quota before you rely on volume:
   `billing.plans()` — the plan catalog, works anonymously.
   `billing.status()` — this tenant's plan and usage against each quota.
7. Inspect and manage: `host.tool_list()`, `host.tool_logs(name)`,
   `host.tool_remove(name)`, `host.usage(window)`,
   `host.secret_set`/`host.secret_list()` (secrets stored AES-256-GCM
   encrypted).

<!-- cite: docs/benchmarks/measure-0.26.3-20260908T085001Z.md -->
<!-- agent-quickstart:end -->

`mcphost serve` is a streamable-HTTP MCP server, stateless per the 2026-07-28
specification, on which an agent signs up with one unauthenticated tool call,
receives a tenant key, and then owns a namespace of tools it publishes, lists,
inspects and removes through further tool calls. There is no web page. The
operator administers tenants and reads metering through `admin.*` tools on
the same endpoint. Tool *execution* kinds (REST wrappers, code) are separate
PRDs; this one ships the endpoint, tenancy, the control plane, the `Kind`
trait, and a built-in `echo` kind so the harness can measure the bootstrap
path end to end.

> The machine-readable summary lives at [`/llms.txt`](https://mcphost.dev/llms.txt)
> on the production endpoint — generated from the same source as the
> quickstart above (`docs/agent-quickstart.md`, `scripts/gen-agent-docs.sh`).

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

### Python spec-language notes

PRD-mcphost-python-kind-runtime (AC6): the AST-check that gates
`host.tool_publish` accepts assignment expressions (`:=`, PEP 572) in
general -- CPython has parsed them since 3.8, and mcphost's publish-time
check and the tool's own runtime both compile `source` with the same
CPython grammar, so there is no mcphost-added restriction to relax. The
one thing that *is* rejected is a restriction Python's own grammar
enforces: an assignment expression's target must be a plain name.
`(obj.attr := 1)` and `(d[key] := 1)` are both invalid Python syntax
(`cannot use assignment expressions with attribute` / `...with
subscript`) and would fail identically whether or not mcphost validated
them first -- the tool's own `main(args)` would refuse to even parse.
Because this is executor-level, not validator-level, there is nothing for
mcphost to loosen; the fix here is that the publish-time rejection now
names the construct and the accepted alternative in one sentence (assign
to a plain name first, then set the attribute/subscript in a separate
statement) instead of leaving CPython's bare grammar message to speak for
itself.

## Call limits

PRD-mcphost-call-limits-honest: every limit here is the one the code
enforces -- `tests/limits_ac06_quickstart_docs_match_constants.rs` checks
this section and `www/llms.txt`'s "Limits and pricing" section against the
same constants `host.quickstart`'s `limits` object reads.

- **Call timeout**: 30 s by default, or your own `timeout_s` up to 60 s max
  -- a python spec that declares `timeout_s` gets exactly that deadline
  (bounded by the 60 s host maximum), not a shorter one applied silently
  underneath it. `call_timeout` names the deadline that actually applied.
- **Output size**: tool output at most 1 MiB. Over the cap returns
  `tool_output_too_large` naming `limit_bytes` and the `actual_bytes`
  produced, never a bare `tool_output_invalid` parse failure.
- **Request body**: at most 1 MiB (HTTP 413 over that -- see
  `tests/ac16_request_body_too_large.rs`; the "2 MiB" in that AC's own
  description is the oversized test payload used to *prove* the 1 MiB cap,
  not the cap itself).
- **Concurrency**: 20 concurrent calls host-wide; per tenant, 4 per tenant on the free plan
  (10 on pro). A refusal past your own tenant's cap is `capacity` with
  `scope: "tenant"` and a `retry_after_ms`; past the host-wide cap it's
  `scope: "host"`.
- **Sandbox process cap**: a python tool's sandbox allows at most 64 live
  processes; a fork past that fails with the structured
  `tool_process_limit`, not a silent hang or an opaque OS error.

## Metered overage (billing emit-meter)

PRD-mcphost-metered-overage: pro tenants' successful calls past the plan's
50,000 included calls/month bill themselves through Stripe's
`mcphost_tool_calls` meter and its graduated metered price. Set these
env vars from `~/.config/mcphost/stripe-objects.json` (unset means the
same v0.14.0 behavior -- no metering, no `meter_lag`):

- `MCPHOST_STRIPE_METERED_PRICE_ID` -- the metered price id `billing.checkout`
  attaches alongside the base price.
- `MCPHOST_STRIPE_METER_EVENT_NAME` -- defaults to `mcphost_tool_calls`.

Then run `mcphost billing emit-meter` on a timer (every five minutes is the
shipped default): it reads pro tenants' unemitted `ok` calls, POSTs one
Stripe meter event per tenant (chunked at 100 events/request), ledgers each
batch, and advances its own high-water mark only once every event in the
run has been accepted -- safe to rerun after a crash or a failed POST (see
`src/metering.rs`'s doc comment for the replay/idempotency contract).

Install the shipped systemd **user** units (`~/.config/systemd/user/`,
matching this host's other `mcphost-*` units):

```
cp deploy/mcphost-emit-meter.service deploy/mcphost-emit-meter.timer \
   ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now mcphost-emit-meter.timer
```

`mcphost-emit-meter.service` reads `~/.config/mcphost/emit-meter.env` (via
`EnvironmentFile=-`, so a missing file is not an error) for
`MCPHOST_DATA_DIR` / `MCPHOST_STRIPE_SECRET_KEY` / the two vars above.
Both unit files pass `systemd-analyze verify --user` (AC9;
`tests/metering_ac09_deploy_units_verify.rs`).

`/healthz`'s `meter_lag` field (present only when `MCPHOST_STRIPE_METERED_PRICE_ID`
is set) is the count of pro-tenant `ok` calls still above the high-water
mark -- watch it for emission health at a glance.

## Synthetic tenants (`admin.*_synthetic`)

PRD-mcphost-synthetic-flag: a tenant a test harness creates carries a
free-form `synthetic` label (e.g. `synthorg:<run_id>`) from signup onward,
set by the harness sending `x-mcphost-synthetic: <label>` on its `signup`
call -- no behavior change, metadata only. `/healthz`'s `tenants_real` /
`tenants_synthetic` split, and `admin.tenants`' `synthetic` filter
(`true`/`false`/`all`, default `all`), read this column so
`synthorg candidates --measure` can exclude panel traffic from "real
tenant" evidence.

**Backfilling the existing census** (every tenant predates this column, so
all load with `synthetic: null` until tagged): use `admin.tenants_set_synthetic`,
previewed with `dry_run: true` before the `dry_run: false` that applies it.
The recipe this host's own census used:

```
admin.tenants_set_synthetic(name_like: 'joe-%',  label: 'operator',                    dry_run: true)
admin.tenants_set_synthetic(name_like: 'joe-%',  label: 'operator',                    dry_run: false)
admin.tenants_set_synthetic(name_like: '%',      label: 'synthorg:backfill-20260906',  dry_run: true)
admin.tenants_set_synthetic(name_like: '%',      label: 'synthorg:backfill-20260906',  dry_run: false)
```

Run the `joe-*` pass first -- the second call's broader `%` pattern would
otherwise overwrite those rows' label too, since a tenant re-tagged by a
later call simply gets the later label (there is no "already labeled, skip"
guard by design: retagging is how a label ever gets corrected). A single
tenant can be corrected at any time with `admin.tenant_set_synthetic(tenant,
label)` (`label: null` clears it).

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
300 s budget <!-- cite: .github/workflows/ci.yml -->. Which targets go where is derived, not hand-listed:
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
| 11 (P0, non-functional) | 200 concurrent `echo` calls, p95 < 50ms, 0 errors, RSS < 100MiB | `tests/ac11_load_smoke.rs` (`#[ignore]`d — hardware-dependent; run with `cargo test --release --test ac11_load_smoke -- --ignored --nocapture`). Measured on the build box: **p95 = 34.20ms, 0 errors, RSS = 37.3MiB** <!-- cite: docs/benchmarks/ac11-load-smoke.txt --> |
| 12 (P0) | `synthorg consume --preflight <url>` exits 0 | `tests/ac12_preflight.rs` — an always-run in-process half exercises the same two requests `run_preflight` makes; a second half spawns the real `mcphost` binary and the real `synthorg` CLI when available (bare binary or `uv run --project`) and asserts exit 0 |
| 13 (P0) | Mismatched `Mcp-Name` header vs. body is recorded by body name and flagged | `tests/ac13_mcp_name_mismatch_metering.rs` |
| 14 (P0) | Unwritable database: `storage` error, `/healthz` `db_ok: false`, process stays up | `tests/ac14_storage_unwritable.rs` |
| 15 (P0) | A call that never completes times out at the deadline, future dropped | `tests/ac15_call_timeout.rs` |
| 16 (P0) | A request body over the 1 MiB cap (proven with a 2MiB body) is rejected with HTTP 413 | `tests/ac16_request_body_too_large.rs` |
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
