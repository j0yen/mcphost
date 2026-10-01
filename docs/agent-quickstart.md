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
   to 5 per IP per hour. Pass `source` (e.g. `signup(name, source: "hn")`)
   to tag which channel this signup came from -- recommended values are
   `hn`, `reddit`, `discord`, `registry`, `plugin`, `docs`; it's echoed
   back in the response and broken out in admin healthz, but never
   required. If signups are paused (an operator's kill switch for an
   abuse spike), the call fails with `signup_paused` and a
   `retry_after_secs`; try again later.
   Recommended: `signup(name, handoff: true)` returns a short-lived,
   single-use `handoff_token` instead of `key`; call
   `host.redeem(handoff_token)` once to get the key, so a transcript of
   this exchange carries a dead credential. `host.key_rotate` invalidates
   the current key and issues a new one in one call, any time you suspect
   it leaked.
3. Pass `key` as the `tenant_key` argument on every `host.*` call from
   here on -- e.g. `host.tool_publish`, `host.tool_call`. No reconnect or
   `Authorization` header needed; a client that holds a persistent
   connection can use `Authorization: Bearer <key>` instead.
4. Publish a tool: `host.tool_publish(name, kind, spec)`. Call
   `host.quickstart` first — its `starter_tool` is a ready-to-publish
   `python` spec (reverses text, counts words) plus the exact
   `publish_call`/`test_call` to run; the documented first publish is a
   real tool, not a stub. Two real kinds: submit code (`python` — source
   required, `args_schema`/`requirements` inferred if omitted) or wrap an
   API you already use (`http` — url and method required, `args_schema`
   inferred if omitted). `echo` (returns its arguments; spec is a JSON
   Schema) is a stub for testing the pipes, not a real tool — it carries
   `stub: true` in `tools/list`. Before publishing anything, dry-run with
   `host.tool_publish({..., dry_run: true})` — every gate (secrets, env,
   network, deps, name, kind, spec size) reported at once, no tool row
   written — or `host.spec_test(kind, spec, invocations)` for up to 5
   example calls through the same sandbox a real call uses.
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
   encrypted). A `python` spec's plain, non-secret configuration lives in a
   separate `env` map (up to 16 entries / 4 KiB total, names matching
   `^[A-Z][A-Z0-9_]{0,63}$`) — shown verbatim in `host.tool_test`, unlike
   `secrets`, which stay redacted there.
8. Share a tool with `host.tool_share(name, visibility, group?)` (see "Share
   a tool, not a key" in `www/llms.txt` for the full recipe). Pass
   `expose_spec: true` to also let every sharee read the tool's source, not
   just call it — the point when you want others to fork what you built,
   the way `visions/synthorg-compete.md`'s round-two builders fork
   round-one winners. A sharee reads it with
   `host.tool_spec_shared(tool: "<owner_namespace>.<name>")`, which returns
   `{tool, kind, spec, exposed_at}` — `spec` never carries `env` or a secret
   reference, only `source`/`args_schema`/`requirements`/`timeout_s`/
   `network`. Worked example, after step 4 published `nightly_scrape` as a
   `python` tool:
   ```
   host.group.create(name="arena-builders")
   host.group.add(name="arena-builders", namespace="<their_namespace>")
   host.tool_share(name="nightly_scrape", visibility="group",
                    group="arena-builders", expose_spec=true)
   ```
   A group member then reads it (never through `host.tool_call`, which only
   runs it) with:
   ```
   host.tool_spec_shared(tool="<your_namespace>.nightly_scrape")
   # -> {"tool": "<your_namespace>.nightly_scrape", "kind": "python",
   #     "spec": {"source": "...", "args_schema": {...}}, "exposed_at": "..."}
   ```
   Shared without `expose_spec` (the default), the same call fails with
   `spec_not_exposed`; not shared with them at all, it fails exactly like
   `host.tool_call` would — `tool_not_found`, never revealing the tool
   exists.

<!-- cite: docs/benchmarks/measure-0.26.3-20260908T085001Z.md -->

## Leaving

`host.self_offboard()` permanently closes your own tenant: no operator
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

## Contributing: naming a new `host.*` tool

mcphost-polish-p0-20260930 (audit finding 5): the registry mixes
`host.<namespace>.<verb>` (dotted — `host.agent.lookup`, `host.docs.put`,
`host.group.create`, `host.channel.post`, `host.msg.send`,
`host.lineage.trace`, `host.enduser.revoke`, ...) with
`host.<noun>_<verb>` (underscored — `host.key_rotate`, `host.bridge_test`,
`host.self_offboard`, ...) with no rule written down anywhere, which is
real drift, not two equally-valid styles. Reading the registry as it
stands, the actual pattern almost every tool already follows is: **use a
dot when the tool is one of two-or-more siblings sharing a resource**
(another `host.agent.*`/`host.docs.*`/`host.group.*`/... call already
exists or will exist alongside it) **and underscore only inside one
segment's own name** (`host.lineage.blast_radius`,
`host.enduser.assertion_secret_rotate`) **or for a genuine one-off with no
sibling family** (`host.export`, `host.key_rotate`). The one named
exception is the `host.tool_*` sharing/publish family
(`host.tool_call`/`host.tool_share`/`host.tool_list`/`host.tool_remove`/
`host.tool_logs`/`host.tool_rollback`/`host.tool_diff`/`host.tool_history`/
`host.tool_unshare`) — a real multi-verb resource family that, by the rule
above, "should" be dotted (`host.tool.call`, ...) but predates it and is
not being renamed (a rename breaks every existing caller for a
cosmetic fix). Do not use `host.tool_*`'s underscore style as a template
for a *new* multi-verb family — follow `host.agent.*`/`host.docs.*`
instead. A PRD to actually reconcile `host.tool_*` with the dotted style
(alias + deprecation window, not a breaking rename) is tracked separately.

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
