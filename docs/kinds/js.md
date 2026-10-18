only source is required -- args_schema is inferred from the `args.<name>` reads (and any JSDoc `@param {type} args.<name>`) in it; source must define `function main(args)` (or `export default`) and is plain JavaScript (ES2020) run by QuickJS in the same sandbox `python` uses.

```json
{
  "source": "function main(args) {\n  return { total: args.items.reduce((a, b) => a + b.price, 0) };\n}\n"
}
```

Call arguments:

```json
{"items": [{"price": 2}, {"price": 5}]}
```

Dry-run before publishing: `host.spec_test("js", spec, invocations)` runs the example invocations above in the same sandbox a real call would use and returns each one's output or a bounded exception, plus the inferred args_schema -- no tool row is written.

A thrown error comes back as `tool_exception` with a message ending `at tool.js:<line>`, and `data.line`, `data.source_line` (that line's text) and `data.families: ["tool_code"]`. A syntax error is refused at publish with `line`, `column` and `source_line`. TypeScript is refused (`typescript_not_supported`, `data.hint` names the `tsc --target es2020` step): publish the emitted JavaScript.

Result envelope contract: identical to `python` -- a call's output lands at `result.payload`, and an optional `outputs` array or object declares fields promoted into `result.payload.<field>`.

Other spec fields: `timeout_s` (seconds the sandboxed call may run, default 10, at most 60), `memory_mb` (megabytes the engine heap may use, default 256), `network` (`"none"` (default), `"public"` or `"egress"`; `fetch` is blocked without a grant), `secrets` and `env` (read as `process.env.SECRET_<NAME>` / `process.env.<NAME>`), and `description`.

## The sandbox API

<!-- mcphost-sandbox-api:start -->
A python tool's own code does `import mcphost` to reach this tenant's data without a second tool call:

- `mcphost.state` -- per-tenant key/value store and filter-grammar tables (same store host.state.* uses)
- `mcphost.table` -- per-tenant SQL tables (same store host.table.* uses)
- `mcphost.docs` -- read or search this tenant's stored documents (same store host.docs.* uses)
- `mcphost.index` -- create, poll and list named indexes (same verbs host.index.* uses)
- `mcphost.blob` -- read or store files of any type under this tenant (same store host.blob.* uses)
- `mcphost.lineage` -- trace an artifact's lineage or estimate a change's blast radius (same report host.lineage.* uses)
- `mcphost.drift` -- list drift review items (same list host.drift.reviews returns)
- `mcphost.channel` -- post to or read a tenant channel (same store host.channel.* uses)
- `mcphost.msg` -- send or read direct messages (same store host.msg.* uses)
- `mcphost.email` -- send an email from this tool's own address (same verb host.email.send is)
- `mcphost.elicit` -- ask the user a typed question mid-call: the client's form when it can, the ask-user link otherwise
<!-- mcphost-sandbox-api:end -->

A js tool reaches this tenant's data through the global `mcphost` object. Its members are generated from the same `BRIDGE_MODULES` table the `python` kind registers, so the two bridges cannot drift: one member per module above, plus `mcphost.call(name, args, timeout_s)` and `mcphost.progress(pct, msg)`. Functions take their python arguments positionally and return the same values; a refusal throws an error named `McphostBridgeError` with `.code` and `.data`. Reading a member that is not a bridge throws `ReferenceError: mcphost.<name> is not a bridge; available: ...`.

```json
{
  "source": "function main(args) {\n  const n = mcphost.state.get(\"count\", 0) + 1;\n  mcphost.state.set(\"count\", n);\n  return { count: n };\n}\n"
}
```

## Limits

`timeout_s` is enforced by the engine itself: a tool that runs past it (`while (true) {}`) is stopped and reported as `tool_timeout`, and the next call starts a fresh process. `memory_mb` is the engine's own heap limit: a tool that grows past it is reported as `tool_oom`. With `network: none` the sandbox has no network namespace route at all, so `fetch(...)` throws an error naming the missing network grant and no socket is opened.

## Spec fields

<!-- kind-fields:start -->
| field | type | required | meaning |
|---|---|---|---|
| `source` | string | yes | JavaScript source defining `main(args)` (or `export default`) |
| `args_schema` | object | no | JSON Schema for the call arguments, inferred from `source` when omitted |
| `timeout_s` | integer | no | seconds the sandboxed call may run |
| `memory_mb` | integer | no | megabytes the sandbox may use |
| `network` | string | no | `none`, `public` or `egress`: what the process may reach beyond the `mcphost.*` bridges |
| `secrets` | array of string | no | `host.secret_set` names injected as `SECRET_<NAME>` environment variables (`process.env`) |
| `env` | object | no | plain, non-secret environment variables (`process.env`) |
| `outputs` | array or object | no | fields a caller can rely on in `result.payload`, optionally with a `$.a.b` path each |
| `description` | string | no | human-readable text shown in `tools/list` |
| `description` | string | no | human-readable text shown in `tools/list` |
| `outputs` | array or object | no | fields a caller can rely on in `result.payload`, optionally with a `$.a.b` path each |
| `reads` | array of string | no | tables this tool reads, recorded for lineage and blast-radius reports |
| `ui` | object | no | optional presentation hints for clients that render a tool's result |
<!-- kind-fields:end -->

## Kind ceilings

<!-- kind-ceilings:start -->
| ceiling | free | pro |
|---|---|---|
| `budget.max_tool_latency_ms` | 30000 | 120000 |
| `budget.max_wall_ms` | 60000 | 600000 |
<!-- kind-ceilings:end -->

## Error codes

<!-- kind-errors:start -->
| code | when |
|---|---|
| `tool_exception` | the tool's code raised an uncaught exception |
| `tool_output_invalid` | the tool's return value did not satisfy its declared outputs |
| `tool_timeout` | the call ran past its `timeout_s` |
| `tool_oom` | the call exceeded its `memory_mb` |
| `tool_process_limit` | the sandboxed process hit its process or file limit |
| `tool_output_too_large` | the tool's output exceeded the result size cap |
| `upstream_auth` | the upstream refused the credentials (401/403) |
| `upstream_rate_limited` | the upstream answered 429 |
| `upstream_client_error` | the upstream answered another 4xx |
| `upstream_error` | the upstream answered 5xx |
| `upstream_timeout` | the upstream did not answer within the call's timeout |
| `upstream_unreachable` | the upstream could not be reached |
| `upstream_not_connected` | the named upstream provider is not connected for this tenant |
| `host_not_allowed` | the target host is not on the allowed list or resolves to a private address |
| `compose_self_call` | a tool named itself as one of its own children |
| `compose_depth_exceeded` | composition nested deeper than 4 levels |
| `compose_children_exceeded` | one top-level call's tree exceeded 50 child calls |
| `compose_item_outside_for_each` | `$.item` or `$.index` used in a step with no `for_each` |
| `compose_mapping_type` | a mapping resolved to the wrong type (for example a non-array `for_each`) |
| `compose_mapping_missing` | a mapping path resolved to nothing |
| `compose_fanout_exceeded` | a `for_each` array was longer than the plan's `chain_fanout_max` |
| `compose_fanout_item_failed` | an item of a `for_each` step failed |
| `compose_input_missing` | a `$.input.<name>` mapping named an argument the call did not carry |
| `tool_building` | the tool's dependencies are still building |
| `tool_not_found` | a named tool does not exist for this tenant |
| `tool_run_unsupported` | the tool cannot be run this way (for example a dry run) |
<!-- kind-errors:end -->

## Canonical example

<!-- kind-example:start -->
```json
{
  "source": "function main(args) {\n  return { total: args.items.reduce((a, b) => a + b.price, 0) };\n}\n"
}
```
<!-- kind-example:end -->

<!-- kind-contract:start -->
## Contract

| Field | Value |
|---|---|
| budget `timeout_s` | 1-60 (default 10) |
| budget `memory_mb` | 1-1024 (default 256) |
| family `tool_code` | tool_exception, tool_output_invalid |
| family `tool_limit` | tool_timeout, tool_oom, tool_process_limit, tool_output_too_large |
| family `upstream` | upstream_auth, upstream_rate_limited, upstream_client_error, upstream_error, upstream_timeout, upstream_unreachable, upstream_not_connected, host_not_allowed |
| family `compose` | compose_self_call, compose_depth_exceeded, compose_children_exceeded, compose_item_outside_for_each, compose_mapping_type, compose_mapping_missing, compose_fanout_exceeded, compose_fanout_item_failed, compose_input_missing |
| family `tool_state` | tool_building, tool_not_found, tool_run_unsupported |
| source_line | yes (`data.source_line`) |
| bridges | state, table, docs, lineage, channel, msg |
| secret redaction | yes |
<!-- kind-contract:end -->
