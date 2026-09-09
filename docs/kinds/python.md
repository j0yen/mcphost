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

`outputs` also accepts an object mapping each field name to a path (`"outputs": {"score": "$.data.score"}`, the same `$.a.b[0].c` dotted/indexed grammar `http` accepts -- no wildcards, filters, or recursive descent); the path parses and publishes, but this kind still promotes every declared field by name only (the wrapper search above) -- reading `main`'s return value at the declared path is not yet implemented.
