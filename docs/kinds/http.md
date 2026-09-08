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

Result envelope contract: an optional `outputs` array of field names (`"outputs": ["bridge_status"]`) declares fields a caller can rely on finding at `result.payload.<field>`, regardless of how deep the upstream body actually nests them -- one level of common wrapping (`data`, `result`, `response`) is searched automatically. `result.body` keeps the full, unmodified upstream response either way. Run `host.tool_test` after publishing to see any declared field your upstream never emits (`envelope.missing`).
