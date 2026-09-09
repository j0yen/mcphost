Runs a fixed, ordered sequence of this tenant's own tools, passing each step's mapped result into the next -- the declarative form of `mcphost.call` (see the `python` kind's own doc), for a pipeline that needs no python of its own.

```json
{
  "steps": [
    {"tool": "fetch_rows", "args": {"since": "$.input.since"}},
    {"tool": "write_rows", "args": {"rows": "$.prev.result.rows"}}
  ]
}
```

Call arguments:

```json
{"since": "2026-01-01"}
```

Each step names a `tool` (unqualified, same tenant) and an `args` object mapping that step's own call arguments -- each value is either a literal JSON value or (a string starting with `"$."`) a path resolved against `{"input": <the chain's own call args>, "prev": <the previous step's result, or null on the first step>, "steps": [<every earlier step's result, by 0-based index>]}`. The grammar is the same dotted/indexed `$.a.b[0].c` paths `outputs` accepts elsewhere in this host -- no wildcards, filters, or recursive descent. A step whose mapping resolves to nothing fails the whole call with `error_code: compose_mapping_missing`, naming the path and `failed_step`; add `"on_error": "continue"` to a step to let the remaining steps run anyway (the parent call still ends `error`-adjacent, but names every `failed_steps` index rather than stopping at the first).

The chain's own result is its last step's result, unless that step itself failed and every later step tolerated its own failure -- then it's the last step that actually ran. `host.tool_test` on a published chain dry-runs it: each step's resolved arguments are reported (`$.prev`/`$.steps[i]` mappings show as `unresolved_path` before anything has actually run) and no step executes.

A chain step goes through the same composition primitive `mcphost.call` uses (`kinds::compose_call`): a chain naming itself as one of its own steps is refused with `error_code: compose_self_call`; nesting chains (or python tools calling chains, or chains calling python tools that call further tools) more than 4 levels deep is refused with `error_code: compose_depth_exceeded`; a single top-level call's whole tree is capped at 50 child calls total (`error_code: compose_children_exceeded`).
