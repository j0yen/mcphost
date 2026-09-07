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
