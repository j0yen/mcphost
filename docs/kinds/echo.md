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
