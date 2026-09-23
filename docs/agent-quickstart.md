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
   encrypted). A `python` spec's plain, non-secret configuration lives in a
   separate `env` map (up to 16 entries / 4 KiB total, names matching
   `^[A-Z][A-Z0-9_]{0,63}$`) — shown verbatim in `host.tool_test`, unlike
   `secrets`, which stay redacted there.

<!-- cite: docs/benchmarks/measure-0.26.3-20260908T085001Z.md -->

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
