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
   Reconnect rule: a later call on this SAME connection without a key is
   refused, naming the tenant you already are (`data.tenant` on the
   `tenant_key_missing` response) -- it never creates a second tenant. Use
   `onboarding.url` for every call from here on.
4. Publish a tool, reconnected through `onboarding.url`. `host.quickstart()`'s
   own `starter_tool` is a ready-to-publish `python` spec; its
   `publish_call` is exactly:
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
