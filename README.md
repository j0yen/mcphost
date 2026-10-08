# mcphost

where agents host their own tools · [mcphost.dev](https://mcphost.dev) · [status](https://mcphost.dev/status.html) · [llms.txt](https://mcphost.dev/llms.txt) · [llms-install.md](https://mcphost.dev/llms-install.md)

Find it in the registry: [dev.mcphost/mcphost](https://registry.modelcontextprotocol.io/v0/servers?search=mcphost) — connect with the one-URL quickstart at [mcphost.dev](https://mcphost.dev).

<!-- agent-quickstart:start -->
Ship an MCP tool, not a deployment project.

mcphost lets an agent create the tool it needs, mid-task, without a human
in the loop: sign up with one unauthenticated tool call, publish with the
next, and the new tool is live immediately — no restart, no deploy, no
review queue.

**Measured** (panel run `0.26.3-20260908T085001Z`, 21 sessions): median
time from signup to a tenant's first successful `host.tool.publish` is
**30.7s**; median time from signup to a successful call on that tenant's
own tool is **42.4s**.
<!-- cite: docs/benchmarks/measure-0.26.3-20260908T085001Z.md -->

## Quickstart for agents

1. Connect to `https://mcphost.dev/mcp` -- paste that URL into your MCP
   client's config. No signup call, no credentials, nothing to paste into
   a header.
2. Make your first call. `host.quickstart()` is read-only and hands back
   a worked example before you commit to anything; any other `host.*`/
   `billing.*` call creates your tenant right then, on this connection,
   with no `signup` call at all -- `host.whoami()` is the simplest:
   ```
   host.whoami()
   ```
3. Read `onboarding.url` once, in that same response, and save it -- it's
   this tenant's own address and its credential in one; reconnecting
   through it later is how you come back as this same tenant.
   `onboarding.memory_hint` names where agents conventionally keep it.
4. Publish a tool. `host.quickstart()`'s own `starter_tool` is a
   ready-to-publish `python` spec; its `publish_call` is exactly:
   ```
   host.tool_publish(name="table_note", kind="python", spec={"source": "import mcphost\n\ndef main(args):\n    mcphost.table.create(name='quickstart_notes', columns={'note': 'text'})\n    mcphost.table.append(table='quickstart_notes', rows=[{'note': args.get('note', '')}])\n    return {'appended': 1}\n"})
   ```
   Two real kinds: submit code (`python` -- source required,
   `args_schema`/`requirements` inferred if omitted) or wrap an API you
   already use (`http` -- url and method required, `args_schema` inferred
   if omitted). `echo` is a stub for testing the pipes, not a real tool.
   Before publishing anything, dry-run with `host.tool_publish({...,
   dry_run: true})` -- every gate (secrets, env, network, deps, name,
   kind, spec size) reported at once, no tool row written -- or
   `host.spec_test(kind, spec, invocations)` for up to 5 example calls
   through the same sandbox a real call uses.
5. Call it -- `host.tool_test(name, args)` dry-runs an already-published
   tool by name, or call it directly as `<namespace>.<tool_name>` (its
   own entry in your `tools/list`):
   ```
   host.tool_test(name="table_note", args={"note": "hello"})
   ```
6. Share it with a teammate: `host.invite.create()` returns an
   `/i/<code>/mcp` URL -- whoever connects there and makes their own
   first call gets a fresh tenant already in contact with you, with any
   tool names passed as `share` already shared (see "Share a tool, not a
   key" in `www/llms.txt` for sharing one tool while keeping its own key
   hidden):
   ```
   host.invite.create()
   ```
7. Relay the claim link to your human: hand them `onboarding.url` and the
   sentence "Claim this backend so it belongs to you: `<url>`." Never
   print the personal URL or the key anywhere but that one handoff.

Also useful once you're past the first call: `billing.plans()`/
`billing.status()` for plan and quota; `host.tool.list()`/
`host.tool.logs(name)`/`host.tool.remove(name)`/`host.usage(window)` to
inspect and manage; `host.secret.set`/`host.secret.list()` for
credentials a published tool needs (secrets stored AES-256-GCM encrypted,
with a separate plain `env` map alongside for everything that isn't one).
Pass `expose_spec: true` on `host.tool.share` (or `share` names on
`host.invite.create`) to also let a sharee read a tool's source, not
just call it -- they read it with
`host.tool.spec_shared(tool="<owner_namespace>.<name>")`; shared without
`expose_spec`, the same call fails `spec_not_exposed` (see "Share a tool,
not a key" in `www/llms.txt` for the full worked example).

<!-- cite: docs/benchmarks/measure-0.26.3-20260908T085001Z.md -->

## Explicit signup (clients that cannot keep a session)

A client that can't hold a persistent connection across calls -- so there
is no session for mcphost to implicitly bind a tenant to -- signs up
directly instead of relying on its first call to create one:

1. Connect to the endpoint and call `tools/list` with no credentials. The
   only tool offered is `signup`.
2. Call `signup(name)`. The response contains `tenant`, `key` (a bearer
   token, shown once), `namespace`, `endpoint`, and `claim_url`. Signup is
   rate-limited to 5 per IP per hour. Pass `source` (e.g.
   `signup(name, source: "hn")`) to tag which channel this signup came
   from -- recommended values are `hn`, `reddit`, `discord`, `registry`,
   `plugin`, `docs`; it's echoed back in the response and broken out in
   admin healthz, but never required. If signups are paused (an
   operator's kill switch for an abuse spike), the call fails with
   `signup_paused` and a `retry_after_secs`; try again later.
   Recommended: `signup(name, handoff: true)` returns a short-lived,
   single-use `handoff_token` instead of `key`; call
   `host.redeem(handoff_token)` once to get the key, so a transcript of
   this exchange carries a dead credential. `host.key.rotate` invalidates
   the current key and issues a new one in one call, any time you suspect
   it leaked.
3. Pass `key` as the `tenant_key` argument on every `host.*` call from
   here on -- e.g. `host.tool.publish`, `host.tool.call`. No reconnect or
   `Authorization` header needed; a client that holds a persistent
   connection can use `Authorization: Bearer <key>` instead.

## Leaving

`host.self.offboard()` permanently closes your own tenant: no operator
ticket, no admin key, no arguments, no confirmation flag — the same
`tenant_key` channel you signed up through is the one you leave through,
and the call takes effect immediately.

What it does, in order: cancels any active Stripe subscription if you're
on the `pro` plan, disables the tenant (every call with this `tenant_key`
after this point — `host.*` or `billing.*` — gets the same
`tenant_disabled`/`tenant_key_invalid` error an admin-disabled tenant
already gets), then gives every registered kind a chance to tear down
whatever it's keeping alive for you (e.g. a `python` sandbox's warm pool).

What it does NOT do: scrub your data. `tools`, `secrets`, `signup_events`,
and usage history all stay in place for audit — exactly the same
retention an admin-disabled tenant gets today. If you want a copy of what
you built before leaving, run `host.export()` first; `self_offboard`
doesn't bundle one for you.

Irreversibility: calling it twice is a no-op, not an error or a crash —
but there is no self-service undo. A canceled Stripe subscription stays
canceled, and your `tenant_key` stops authenticating the instant the call
returns. (An operator can flip the underlying tenant row back on with
`admin.tenant_enable`, but that's an operator action taken on your behalf,
not something `self_offboard` itself offers back to you.)

## Getting help

Every error payload carries `code`, a clean `message`, a `request_id`, and
(for every code in the table below) a `help_url` pointing at a generated
`/help/<code>` page -- meaning, likely cause, fix, no internal text.
`host.whoami`'s `links` field names the same `support`/`plans`/`status`/
`help` pages directly, so an agent never has to guess the host to build
them from.

<!-- support:start -->
Support: support channel not configured (MCPHOST_SUPPORT_URL is unset).
<!-- support:end -->

## Contributing: naming a new `host.*` tool

One rule now, written down and enforced by CI: `host.<family>.<verb>` --
see [`docs/tool-naming.md`](tool-naming.md) for the rule, its exceptions
(`signup`/`billing.*`, and singleton nouns like `host.whoami`), and how
aliases work. `host.tool.call`/`host.tool.share`/`host.tool.list`/
`host.tool.remove`/`host.tool.logs`/`host.tool.rollback`/`host.tool.diff`/
`host.tool.history`/`host.tool.unshare` and the other pre-rule underscored
names (mcphost-polish-p0-20260930 audit finding 5) are no longer drift:
each is registered under its dotted canonical name
(`host.tool.call`/`host.tool.share`/...) and kept working under its old
name as a deprecated alias (sunset date in `tools/list`'s own
`x-deprecated`) -- `scripts/tool-naming-lint.sh` fails CI on any *new*
name that doesn't follow the rule.

## Contributing: routing a new top-level path

Adding a new top-level directory or file to this repo (like `www/`,
`deploy/`, or `.buildloop/`) needs a matching `[[lane]]` entry in
`agent/proof-lanes.toml`, in the same PR that adds the path — `autobuilder
vti-plan` (the branch gate's routing check) refuses any changed path that
resolves to zero lanes, and `tests/lanecov_ac01_every_tracked_path_routes.rs`
enforces the same rule locally via `cargo test`, so a missing lane fails
fast instead of turning every branch gate red after the path lands on
`main`. Give the lane an `id`, a one-line `description` naming the PRD or
reason the path exists, `globs` covering the new path (`"<dir>/**"` for a
directory), and `required_commands` — the cheapest command that actually
proves a change under that path, not necessarily the full test suite. The
`loop-config` lane (routing `.buildloop/**` to `cargo test --workspace`) is
a worked example: one glob, one required command, added in the same PR
that made the path matter.

## Docs Q&A in a minute

Turn a folder of markdown into a checkable Q&A tool, end to end, in one
script:

1. `signup(name)` — one fresh tenant.
2. `host.docs.put(name, content)` — once per document (this recipe's own
   corpus: 8 documents, ~22 KiB total, well under the 2 MiB per-document
   cap).
3. `host.docs.status()` — poll until `index.lag_seconds == 0` (usually one
   or two of the indexer's own 10s ticks; this recipe's corpus is ready
   well under 30s).
4. `host.tool_publish(name="ask_docs", kind="python", spec={"source": ...})`
   — the one published tool. Its `main` calls `mcphost.docs.search(query,
   k)` over the same zero-network sandbox loopback `mcphost.docs.get`
   already uses, so answering a question never spends a public tool call.
5. `<namespace>.ask_docs(query="...")` — ask it. Every passage in the
   result carries a `name:offset` citation (the document's name and its
   character offset in that document), so an answer is checkable against
   its source.

Quota this recipe uses: 8 documents, ~22 KiB, ~30 chunks, 1 published
tool, up to 10 calls to that tool — comfortably inside the free plan's
`docs_max` (200) and `calls_per_day` (500).

`examples/docs-qa/docs-qa.sh <endpoint>` runs this recipe end to end
against a real endpoint and writes a receipt (per-question hit and
citation, `index_ready_secs`, the quota actually used);
`examples/docs-qa/ask_docs.py` is the tool itself, and
`examples/docs-qa/corpus/` is the 8-document corpus plus its 6 gold
questions. `docs-qa.sh --embeddings <provider-endpoint> <model>
<secret-name>` configures an embeddings provider first and reports both
lexical and embeddings hit rates in one receipt.
<!-- agent-quickstart:end -->

## What it is

`mcphost serve` is one Rust binary that speaks streamable-HTTP MCP at a single endpoint. An agent signs up with one unauthenticated tool call, gets a namespace, and from then on everything is a tool call: publish, run, schedule, log, meter, store secrets, share with another agent, act as one of your end users over OAuth. There is no dashboard; `/status` is the one page, and the operator works through `admin.*` tools on the same endpoint.

<!-- cite: docs/benchmarks/ac11-load-smoke.txt -->
Measured, not promised: p95 34.2 ms across 200 concurrent calls with zero errors, committed next to the test that produces it. The public site is [mcphost.dev](https://mcphost.dev); the machine-readable summary at [`/llms.txt`](https://mcphost.dev/llms.txt) is generated from the same source as the quickstart above (`docs/agent-quickstart.md`, `scripts/gen-agent-docs.sh`).

## Connect

Paste one line into your client and it has mcphost. No key needed to sign
up -- `signup` is the one unauthenticated tool; everything past it takes
the bearer key `signup` returns.

<!-- install-links:start -->
**Claude Code**

```
claude mcp add --transport http mcphost https://mcphost.dev/mcp
```

Docs: <https://code.claude.com/docs/en/mcp> (checked 2026-10-07)

**Cursor**

[Add to Cursor](cursor://anysphere.cursor-deeplink/mcp/install?name=mcphost&config=eyJ1cmwiOiJodHRwczovL21jcGhvc3QuZGV2L21jcCJ9), or add `https://mcphost.dev/mcp` to your MCP config directly.

Docs: <https://docs.cursor.com/en/tools/mcp> (checked 2026-10-07)

**VS Code**

[Add to VS Code](vscode:mcp/install?%7B%22name%22%3A%22mcphost%22%2C%22type%22%3A%22http%22%2C%22url%22%3A%22https%3A%2F%2Fmcphost%2Edev%2Fmcp%22%7D), or add `https://mcphost.dev/mcp` to your MCP config directly.

Docs: <https://code.visualstudio.com/api/extension-guides/ai/mcp> (checked 2026-10-07)

**Claude.ai**

1. Open Settings, then Connectors, then Add custom connector.
2. Name: mcphost
3. Remote MCP server URL: https://mcphost.dev/mcp
4. Save, then enable the connector in a chat to connect.

Docs: <https://support.claude.com/en/articles/11175166-get-started-with-custom-connectors-using-remote-mcp> (checked 2026-10-07)

**Codex CLI**

```
codex mcp add mcphost --url https://mcphost.dev/mcp
```

Docs: <https://developers.openai.com/codex/mcp> (checked 2026-10-07)

**Gemini CLI**

```
gemini mcp add --transport http mcphost https://mcphost.dev/mcp
```

Docs: <https://geminicli.com/docs/tools/mcp-server/> (checked 2026-10-07)

**OpenCode**

```
opencode mcp add mcphost --url https://mcphost.dev/mcp
```

Docs: <https://opencode.ai/docs/mcp-servers/> (checked 2026-10-07)

**Amp**

```
amp mcp add mcphost https://mcphost.dev/mcp
```

Docs: <https://ampcode.com/manual#mcp> (checked 2026-10-07)

**Goose**

1. In a Goose session, type /extension.
2. Choose Add Remote Extension (Streamable HTTP).
3. Name: mcphost
4. Endpoint URL: https://mcphost.dev/mcp

Docs: <https://block.github.io/goose/docs/getting-started/using-extensions/> (checked 2026-10-07)

**Warp**

1. In a Warp agent session, type /agent-add-mcp.
2. Paste this server config: {"mcphost":{"url":"https://mcphost.dev/mcp"}}
3. Save; Warp starts the server and lists its tools.

Docs: <https://docs.warp.dev/agent-platform/capabilities/mcp> (checked 2026-10-07)

**Windsurf**

```json
{
  "mcpServers": {
    "mcphost": {
      "url": "https://mcphost.dev/mcp"
    }
  }
}
```

Docs: <https://docs.windsurf.com/windsurf/cascade/mcp> (checked 2026-10-07)

**Cline / Roo**

```json
{
  "mcpServers": {
    "mcphost": {
      "type": "streamableHttp",
      "url": "https://mcphost.dev/mcp"
    }
  }
}
```

Docs: <https://docs.cline.bot/mcp/configuring-mcp-servers> (checked 2026-10-07)
<!-- install-links:end -->

See the live [status page](/status.html) and the
[Acceptable Use Policy](/aup.html) before you point production traffic at
it. Plans and limits: [plans](/plans.html) / [`/plans.json`](/plans.json)
(same numbers `billing.plans` returns, and `docs/plans.md`). Every error
payload carries a `help_url` pointing at a generated `/help/<code>` page
explaining it.

<!-- support:start -->
Support: support channel not configured (MCPHOST_SUPPORT_URL is unset).
<!-- support:end -->

## Changes

Current release: v0.64.0 (2026-09-30); `main` is 0.65.0. Every change is in [`CHANGELOG.md`](CHANGELOG.md) and the git tags.

## Operating and contributing

Everything below is for running your own mcphost or changing this one: install, environment, kinds, limits, metering, synthetic tenants, and the acceptance suite.

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
| `MCPHOST_SIGNUP_RATE_LIMIT_PER_HOUR` | Overrides the per-source-IP `signup` rate limit (PRD-mcphost-signup-rate-configurable) — raise it for a many-session measure run from one IP; absent or non-integer falls back to the default. Effective value is logged once at startup. A source IP configured in `MCPHOST_FLEET_IPS` is exempt from this limit entirely | `5` |
| `MCPHOST_EGRESS_PROXY` | `http(s)://host:port` of the operator's outbound HTTP(S) proxy. Required for a `pro` tenant's `python`/`wasm` tool published with `network: "public"` or `"egress"` to get any sandbox network at all — see "Egress proxy" below | unset (no `pro` tenant gets outbound network) |
| `MCPHOST_FLEET_IPS` | Comma-separated list of IPv4/IPv6 addresses and/or CIDR blocks (e.g. `46.225.110.44,178.105.64.66,10.0.0.0/8`) this operator's own fleet signs up from. A signup whose source IP matches gets `source_class: fleet` (`synthetic: harness:fleet-ip`) even without the `x-mcphost-synthetic` header — invalid entries are logged and skipped. See `admin.reclassify_fleet_ips` to backfill signups that predate this var | unset (no IP is ever classified `fleet` by address alone) |

`mcphost migrate` applies pending SQL migrations and exits. `mcphost version`
prints the version and exits. `mcphost serve --registry-url <url>` is the
CLI-flag form of `MCPHOST_REGISTRY_URL` above.

### Egress proxy (`network: "public"` / `"egress"`)

A `free` tenant can never publish a tool with `network: "public"` or
`"egress"` — both spellings grant the same sandbox access, and both are
refused at `host.tool_publish` with `plan_required` naming `pro` and the
`network` field. A `pro` tenant *may* publish one, but the sandbox still
gets no outbound network at call time unless `$MCPHOST_EGRESS_PROXY` is
configured on this host — with it unset, the call fails with
`egress_unavailable` before any sandboxed process is even spawned. An
existing tool that declared `public`/`egress` while its owner was on `pro`
starts failing with `plan_required` (not a silent downgrade to no network)
the moment that tenant drops to `free`; the error names `network: "none"` as
the republish fix, or upgrading back to `pro`.

With the proxy configured, a `pro` tenant's egress call runs with
`--share-net` (its outbound network shares the host's own network
namespace) and `http_proxy`/`https_proxy`/`HTTP_PROXY`/`HTTPS_PROXY` all set
to `$MCPHOST_EGRESS_PROXY` in the sandboxed process's environment, plus
`no_proxy=""` so a stray `NO_PROXY` already in the operator's environment
can never let a sandboxed process route around the proxy. This crate does
not ship the proxy itself (`mcphost-deploy` owns that config) — it only
guarantees "no proxy, no network." Whatever proxy is deployed **must**
enforce a private/link-local deny list on every connection it forwards, the
same ranges the `http` kind's own `VettingResolver` already refuses for a
rendered request URL:

- RFC 1918 private ranges (`10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`)
- `169.254.0.0/16` (link-local, including the cloud metadata address
  `169.254.169.254`)
- `127.0.0.0/8` (loopback) and `::1`
- `fc00::/7` (IPv6 unique local addresses)

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

### This host's own registry listing

Distinct from "Registry publish (P1)" above, which is a tenant-facing
tool for a TENANT's own namespace — this is mcphost's own listing.
`mcphost registry-manifest` builds the public MCP registry's
`server.json` entry from `Cargo.toml` (name under the DNS-verified
`dev.mcphost` namespace, description, version, license) and
`$MCPHOST_PUBLIC_URL` (default `https://mcphost.dev`) + `/mcp` as the
`streamable-http` remote:

- No flags: prints the entry to stdout.
- `--write <path>`: writes it (used to regenerate the committed
  `registry/server.json`, never hand-edited).
- `--check`: compares a committed file (default `registry/server.json`)
  against a fresh render and exits non-zero with a diff on drift --
  except in the `version` field, which it ignores. A release land
  commits `registry/server.json` and only afterwards bumps
  `Cargo.toml`'s version, so the committed file is deliberately
  version-stale between a PRD land and the next tag; `--check` would
  otherwise fail every such land on this file alone, by construction.
  Any other drift (description, name, license, repository, remotes)
  still fails `--check` as before.

`.github/workflows/registry.yml` runs `--check` plus schema validation
against the pinned `registry/schema.json` on every push, and on a `v*`
tag regenerates `registry/server.json` with `--write` (so the published
entry carries the real, just-tagged version even though the committed
copy in the repo stays version-stale) before publishing it to the
registry via the `MCPHOST_REGISTRY_PRIVATE_KEY` repository secret (the
`dev.mcphost` domain-namespace DNS login key for the registry publisher
CLI).

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

### `wasm`

component is a base64-encoded WebAssembly component (component-model, not a core module) exporting `call: func(args: string) -> result<string, string>`; args_schema is optional (defaults to accepting any object).

Example spec:

```json
{
  "component": "AGFzbQEAAAAA"
}
```

Example call arguments:

```json
{
  "msg": "hi"
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
300 s budget <!-- cite: .github/workflows/ci.yml -->. Which SUITE BINARIES go
where is derived, not hand-listed: `scripts/ci-test-partition.sh core|sandbox`
classifies every `tests/*.rs` FILE by whether it touches the sandbox-execution
surface (PRD-mcphost-test-suite-consolidation moved the unit cargo links from
"one binary per file" to a handful of `tests/suite_<core|sandbox>_NN.rs`
binaries — see "Adding a test" below — so the partition is now file→suite,
not file→binary), and `check` proves the split is total and disjoint at both
levels. Both jobs then fail on any capability-skip in their log, so a file
filed into the wrong half turns CI red rather than passing vacuously.

### Adding a test

`tests/*.rs` stopped being cargo's unit of test-binary discovery
(PRD-mcphost-test-suite-consolidation, 2026-09-12): `autotests = false` in
`Cargo.toml`, plus a handful of generated `tests/suite_<core|sandbox>_NN.rs`
files that `#[path]`-include the real files, keep `target/debug/deps` from
holding one ~280 MB binary per test file. Every test keeps its own file, its
own name, and its AC pairing — only which BINARY it links into changed.

To add a test: drop `tests/<name>.rs` in as always (same naming convention:
`<prefix>_ac<N>_<description>.rs`, `mod common;` if it needs the shared
harness), then run `scripts/gen-test-suites.sh` to fold it into a suite (or
just let CI tell you — `scripts/gen-test-suites.sh --check`, wired into
`ci-test-partition.sh check`, fails naming the exact file if you forget). The
generator buckets by filename prefix, splits sandbox-needing files from
core-only ones first (so no suite ever mixes the two — see above), and
rewrites a lone top-level `mod common;`/`mod ci_sandbox_support;` line in your
new file to `use crate::common;`/`use crate::ci_sandbox_support;` (those
compile once per suite now, not once per file) — no other line changes.
Never hand-edit a `tests/suite_*.rs` file; it is fully regenerated.

Running a single test by name now takes one extra flag: `cargo test --test
suite_core_01 my_test_file:: -- --nocapture` (`cargo nextest run -E
'test(my_test_file::)'` works too, and needs no suite name at all). `cargo
test --test my_test_file` alone no longer resolves — that file isn't its own
cargo target anymore.

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
| 11 (P0, non-functional) | 200 concurrent `echo` calls, p95 < 50ms, 0 errors, RSS < 100MiB | `tests/ac11_load_smoke.rs` (`#[ignore]`d — hardware-dependent; run with `cargo test --release --test suite_core_01 ac11_load_smoke:: -- --ignored --nocapture`). Measured on the build box: **p95 = 34.20ms, 0 errors, RSS = 37.3MiB** <!-- cite: docs/benchmarks/ac11-load-smoke.txt --> |
| 12 (P0) | `synthorg consume --preflight <url>` exits 0 | `tests/ac12_preflight.rs` — an always-run in-process half exercises the same two requests `run_preflight` makes; a second half spawns the real `mcphost` binary and the real `synthorg` CLI when available (bare binary or `uv run --project`) and asserts exit 0 |
| 13 (P0) | Mismatched `Mcp-Name` header vs. body is recorded by body name and flagged | `tests/ac13_mcp_name_mismatch_metering.rs` |
| 14 (P0) | Unwritable database: `storage` error, `/healthz` `db_ok: false`, process stays up | `tests/ac14_storage_unwritable.rs` |
| 15 (P0) | A call that never completes times out at the deadline, future dropped | `tests/ac15_call_timeout.rs` |
| 16 (P0) | A request body over the 1 MiB cap (proven with a 2MiB body) is rejected with HTTP 413 | `tests/ac16_request_body_too_large.rs` |
| 17 (P0) | `Kind` conformance suite passes `echo`, fails naming `describe` for a bad schema | `tests/ac17_kind_conformance.rs` (reusable checker at `mcphost::kinds::conformance`) |
| 18 (P1) | `tools/list` carries `ttlMs`/`cacheScope`, `ttlMs: 0` within 60s of a publish | `tests/ac18_tools_list_ttl.rs` |
| 19 (P1) | `host.registry_publish()` + `/.well-known/mcp/<ns>/server.json` | `tests/ac19_registry_publish.rs` (mocks the registry API with `wiremock`; see "Registry publish (P1)" above — the namespace-verification METHOD stays out of scope, "verified" is an admin-set boolean) |

## Related fleet work

- `mcp-core` (private repo, not publicly linkable) — the reusable stdio
  JSON-RPC 2.0 MCP-server core (`Tool` trait + `serve_stdio`) other wintermute
  MCP servers build on. Not reused here: `mcphost` is a streamable-HTTP
  server (`rmcp`), not a stdio server, and its tool surface is dynamic
  (per-tenant, DB-backed) rather than the static `Tool` trait `mcp-core`
  wraps. Cited per the PRD's technical considerations as related, not shared,
  code.

## License

Dual-licensed under MIT OR Apache-2.0 — see `LICENSE-MIT` and
`LICENSE-APACHE`.
