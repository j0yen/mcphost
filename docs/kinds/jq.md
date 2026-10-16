A jq program is a tool: spec.program runs in-process over the call arguments (`.`), and `$prev` holds the previous chain step's result.

```json
{
  "program": "[.rows[] | select(.status != \"ok\") | {id, status}]"
}
```

Call arguments:

```json
{"rows": [{"id": 1, "status": "ok"}, {"id": 2, "status": "failed"}]}
```

A program that does not parse is refused at publish with `line`, `column` and `source_line`. `input`, `inputs`, `env`, `$ENV`, `@sh`, `halt`, `import` and `include` are refused by name (`jq_builtin_unsupported`, listing the allowed builtins). A runtime error is a `tool_exception` with `exception_class: jq_error`. `timeout_s` (1-10, default 2) and `max_output_bytes` (1 KiB-1 MiB, default 256 KiB) bound a call; a single output is the result, several outputs are an array.

Optional fields: `args_schema` (a JSON Schema for the call arguments; defaults to `{"type": "object"}`), `description` (shown to callers) and `outputs` (declared result fields, same shapes as the other kinds).

## Spec fields

<!-- kind-fields:start -->
| field | type | required | meaning |
|---|---|---|---|
| `program` | string | yes | the jq program, run over the call arguments (`.`); `$prev` holds the previous chain step's result |
| `args_schema` | object | no | JSON Schema for the call arguments (defaults to `{"type": "object"}`) |
| `timeout_s` | integer | no | seconds the program may run (1-10, default 2) |
| `max_output_bytes` | integer | no | bytes of output a call may return (1 KiB-1 MiB, default 256 KiB) |
| `description` | string | no | text shown to callers in the tool listing |
| `outputs` | array or object | no | fields a caller can rely on in `result.payload` |
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
| `tool_timeout` | the call ran past its `timeout_s` |
| `tool_output_too_large` | the tool's output exceeded the result size cap |
<!-- kind-errors:end -->

## Canonical example

<!-- kind-example:start -->
```json
{
  "program": "[.rows[] | select(.status != \"ok\") | {id, status}]"
}
```
<!-- kind-example:end -->

<!-- kind-contract:start -->
## Contract

| Field | Value |
|---|---|
| budget `timeout_s` | 1-10 (default 2) |
| budget `max_output_bytes` | 1024-1048576 (default 262144) |
| family `tool_code` | tool_exception |
| family `tool_limit` | tool_timeout, tool_output_too_large |
| source_line | yes (line and column) |
| bridges | none |
| secret redaction | no |
<!-- kind-contract:end -->
