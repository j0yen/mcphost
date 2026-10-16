url must be an absolute https URL; method and url are the only required fields -- args_schema is inferred from the url/header/body templates when omitted.

```json
{
  "method": "GET",
  "url": "https://api.example.com/items/{{id}}"
}
```

Call arguments:

```json
{"id": "123"}
```

Dry-run before publishing: `host.spec_test("http", spec, invocations)` calls the wrapped endpoint through the same outbound path a real call would use and returns each invocation's status code and a bounded body excerpt -- no tool row is written.

Result envelope contract: an optional `outputs` array of field names (`"outputs": ["bridge_status"]`) declares fields a caller can rely on finding at `result.payload.<field>`, regardless of how deep the upstream body actually nests them -- one level of common wrapping (`data`, `result`, `response`) is searched automatically. `result.body` keeps the full, unmodified upstream response either way. Run `host.tool_test` after publishing to see any declared field your upstream never emits (`envelope.missing`, with `missing_detail` naming where else in the body that field name turned up).

`outputs` also accepts an object mapping each field name to the exact path to read it from, when the upstream buries it somewhere the wrapper search above won't find (`"outputs": {"ingestion_status": "$.json.ingestion_status"}` reads that field from `result.body.json.ingestion_status`). A path is `$` followed by dotted keys and bracketed integer indices only (`$.a.b[0].c`) -- no wildcards, filters, or recursive descent; an unsupported or malformed path is refused at publish, naming the field (`outputs.<name>`) and the accepted grammar.

Other spec fields: `headers`/`query` (objects of header/query-parameter names to `{{ jinja }}`-templated values, the same templating `url`/`body` use), `timeout_s` (a positive integer number of seconds, default 10), `description` (a human-readable string shown in `tools/list`), and `upstream_provider` (a string naming a `host.vault.provider_set` provider whose connected end-user token this call uses, instead of a hand-templated `secret.<name>` header).

Publish error `url: host '<host>' is not a publicly callable host` (code `host_not_allowed`): an `http` tool may only call a public https host. A literal IP in a private, loopback or link-local range, `localhost`, any `.internal` name, and the host's own domain (for example `mcphost.dev`) are refused at publish time. To reach another mcphost tool from a tool, use the `python` kind's `mcphost.call` instead of an `http` loop back to the host.

## Spec fields

<!-- kind-fields:start -->
| field | type | required | meaning |
|---|---|---|---|
| `method` | string | no | HTTP method of the upstream request |
| `url` | string | no | upstream URL; `{name}` placeholders are filled from call arguments |
| `headers` | object | no | request headers; values may reference secrets |
| `query` | object | no | query-string parameters |
| `body` | object | no | request body template |
| `args_schema` | object | no | JSON Schema for the tool's call arguments |
| `timeout_s` | integer | no | seconds the upstream call may take |
| `response` | object | no | how to shape the upstream response into the tool result |
| `description` | string | no | human-readable text shown in `tools/list` |
| `upstream` | string | no | name of a connected upstream provider to call instead of `method` + `url` |
| `upstream_provider` | string | no | provider the `upstream` connection belongs to |
| `outputs` | array or object | no | fields a caller can rely on in `result.payload`, optionally with a `$.a.b` path each (every kind) |
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
| `upstream_auth` | the upstream refused the credentials (401/403) |
| `upstream_rate_limited` | the upstream answered 429 |
| `upstream_client_error` | the upstream answered another 4xx |
| `upstream_error` | the upstream answered 5xx |
| `upstream_timeout` | the upstream did not answer within the call's timeout |
| `upstream_unreachable` | the upstream could not be reached |
| `upstream_not_connected` | the named upstream provider is not connected for this tenant |
| `host_not_allowed` | the target host is not on the allowed list or resolves to a private address |
<!-- kind-errors:end -->

## Canonical example

<!-- kind-example:start -->
```json
{
  "method": "GET",
  "url": "https://api.example.com/items/{{id}}"
}
```
<!-- kind-example:end -->

<!-- kind-contract:start -->
## Contract

| Field | Value |
|---|---|
| budget `timeout_s` | 1-30 (default 10) |
| family `upstream` | upstream_auth, upstream_rate_limited, upstream_client_error, upstream_error, upstream_timeout, upstream_unreachable, upstream_not_connected, host_not_allowed |
| source_line | no |
| bridges | none |
| secret redaction | yes |
<!-- kind-contract:end -->
