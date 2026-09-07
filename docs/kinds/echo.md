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
