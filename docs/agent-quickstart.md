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
