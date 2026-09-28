---
description: Run the mcphost docs-qa recipe end to end against a configured endpoint and report the receipt path.
argument-hint: [endpoint]
---

Run the "Docs Q&A in a minute" recipe (see the mcphost skill and
`www/llms.txt`) against the endpoint given as `$1`, defaulting to
`https://mcphost.dev/mcp` when no argument is given:

```bash
examples/docs-qa/docs-qa.sh "${1:-https://mcphost.dev/mcp}"
```

The script signs up a fresh, throwaway tenant, puts its own 8-document
corpus, waits for the index to catch up, publishes `ask_docs`, asks its 6
gold questions, and prints a line of the form `RECEIPT: <path>` naming
where it wrote the run's receipt JSON (per-question hits and citations,
timings, quota used). Report that path back to your human -- never print
the tenant's bearer key, and never print the receipt's contents unless
asked for them.
