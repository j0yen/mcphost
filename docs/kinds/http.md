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
