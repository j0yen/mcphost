only source is required -- args_schema and requirements are both inferred from it (tool-infer, v0.4.0); source must define main(args).

```json
{
  "source": "def main(args):\n    return {\"doubled\": args[\"n\"] * 2}\n"
}
```

Call arguments:

```json
{"n": 3}
```

Dry-run before publishing: `host.spec_test("python", spec, invocations)` runs the example invocations above in the same sandbox a real call would use and returns each one's output or a bounded exception, plus the inferred args_schema and requirements -- no tool row is written.

Result envelope contract: an optional `outputs` array of field names (`"outputs": ["diagnosis"]`) declares fields a caller can rely on finding at `result.payload.<field>`, regardless of how deep `main`'s returned object actually nests them -- one level of nesting under any key is searched automatically. A tool that returns a bare scalar/list instead of an object gets the whole value promoted to the first declared field, with a `result.payload._envelope_warning` naming the scalar promotion. Run `host.tool_test` after publishing to see any declared field your tool never emits (`envelope.missing`, with `missing_detail` naming where else that field name turned up).

`outputs` also accepts an object mapping each field name to a path (`"outputs": {"score": "$.data.score"}`, the same `$.a.b[0].c` dotted/indexed grammar `http` accepts -- no wildcards, filters, or recursive descent); that path is read directly from `main`'s return value (PRD-mcphost-surface-fluidity), the same as `http`'s own path-declared fields -- a bare-name entry in the same `outputs` still falls back to the wrapper-search promotion above.

## mcphost.state (per-tenant memory)

`import mcphost` inside `source` and call `mcphost.state.get/set/delete/list/insert/query/delete_rows/table_create` -- the same store `host.state.*` reads and seeds from the agent's own session, scoped to this tenant, reachable with `network: none`:

```json
{
  "source": "import mcphost\ndef main(args):\n    n = mcphost.state.get(\"count\", 0) + 1\n    mcphost.state.set(\"count\", n)\n    return {\"count\": n}\n"
}
```

`mcphost.state.get(key, default=None)` returns `default` when the key was never set; every other function takes the same arguments as its `host.state.*` counterpart (`table_create(name, schema, primary_key=None)`, `query(table, where=None, order_by=None, limit=None)`, and so on) and raises `mcphost.state.StateError` (with `.code`/`.data`) on a quota or schema violation rather than returning an error value. A call's `mcphost.state` operations are attributed to that call and see its own writes immediately; there is no state visible across tenants.

## mcphost.call (call another tool)

`import mcphost` and call `mcphost.call(name, args, timeout_s=None)` to run another tool in this same tenant as a child call, synchronously, and get its result back -- reachable with `network: none`, over the same channel `mcphost.state` uses:

```json
{
  "source": "import mcphost\ndef main(args):\n    rows = mcphost.call(\"fetch_rows\", {\"since\": args[\"since\"]})\n    return mcphost.call(\"write_rows\", {\"rows\": rows[\"rows\"]})\n"
}
```

`name` is the target tool's own (unqualified) name; `args` is its call arguments; an optional `timeout_s` bounds this one child call further, but never past the caller's own remaining deadline. The child's result comes back exactly as calling it directly would return it -- no envelope wrapper. A failure raises `mcphost.CallError` (`.code`/`.data`, e.g. `error_code: tool_exception` when the child itself raised); left uncaught, it surfaces on the caller's own call the same way any other unhandled exception does. A tool cannot call itself (`compose_self_call`), nesting is capped at 4 levels deep (`compose_depth_exceeded`), and a single call tree may make at most 50 child calls in total (`compose_children_exceeded`) -- every refusal names the limit it hit.

See the `chain` kind for the declarative form of the same idea: an ordered list of tool calls with no python of your own to write.

## env (plain configuration, distinct from secrets)

`"env": {"UPSTREAM_URL": "https://example.test", "MODE": "fast"}` puts plain, non-secret configuration into the sandboxed process's environment beside `secrets` -- an endpoint URL, a mode flag, a tenant identifier: anything that isn't a credential and doesn't need the secret store's encryption, rotation story, or `host.tool_test` redaction.

```json
{
  "source": "import os\ndef main(args):\n    return {\"mode\": os.environ[\"MODE\"]}\n",
  "env": {"MODE": "fast"}
}
```

Bounds, enforced at publish: at most 16 entries, at most 4 KiB total across every name and value combined, and a value must be valid UTF-8 with no NUL byte -- each violation is a structured error naming the key and the bound it broke. A name must match `^[A-Z][A-Z0-9_]{0,63}$`; `MCPHOST_*`, `PATH`, `HOME`, `PYTHON*`, `LD_*`, and `SECRET_*` (this kind's own prefix for injecting a resolved secret) are reserved and refused by name or prefix. An `env` name may never collide with a secret name already set for this tenant, in either direction: publishing `env` that collides with an existing secret is refused, and so is `host.secret_set`ing a secret whose name collides with an already-published tool's `env` entry.

The distinction is visible, not doctrinal: `host.tool_test` renders both in one listing, `env` values shown verbatim and secret values redacted, each labeled `"kind": "env"` or `"kind": "secret"`. `host.tool_list` returns a tool's `env` map to its owner; `admin.tool_list` reports env names and total size to an operator, never values. Updating `env` alone (no source change) takes effect on the tool's next call -- the warm pool re-fingerprints on an env change exactly as it does on a secret change, so a stale pooled process never serves old values.
