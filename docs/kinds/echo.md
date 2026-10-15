spec.schema is any JSON Schema; a call echoes back the arguments it was given, validated against it.

```json
{
  "schema": {
    "type": "object",
    "properties": {"msg": {"type": "string"}},
    "required": ["msg"]
  }
}
```

Call arguments:

```json
{"msg": "hi"}
```

Dry-run before publishing: `host.spec_test("echo", spec, invocations)` runs the example invocations above through the same path a real call would use and returns each one's output verbatim -- no tool row is written.

## Spec fields

<!-- kind-fields:start -->
| field | type | required | meaning |
|---|---|---|---|
| `schema` | object | yes | JSON Schema the call arguments are validated against; a call echoes them back |
| `reads` | array of string | no | tables this tool reads, recorded for lineage and blast-radius reports |
| `ui` | object | no | optional presentation hints for clients that render a tool's result |
<!-- kind-fields:end -->

## Kind ceilings

<!-- kind-ceilings:start -->
No plan ceiling bounds a call to this kind.
<!-- kind-ceilings:end -->

## Error codes

<!-- kind-errors:start -->
This kind returns no kind-specific error codes.
<!-- kind-errors:end -->

## Canonical example

<!-- kind-example:start -->
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
<!-- kind-example:end -->
