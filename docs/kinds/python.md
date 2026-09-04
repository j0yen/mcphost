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
