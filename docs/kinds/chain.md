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

Each step names a `tool` (unqualified, same tenant) and an `args` object mapping that step's own call arguments -- each value is either a literal JSON value or (a string starting with `"$."`) a path resolved against `{"input": <the chain's own call args>, "prev": {"result": <the previous step's result>} (null on the first step), "steps": [{"result": <step's result>}, ...] (every earlier step, by 0-based index)}`. `$.prev` has exactly one key, `result`; `$.steps[i]` has exactly one key, `result`. So the field `action` of the previous step's result is `$.prev.result.action`, never `$.prev.action`; a mapping that omits `result`, or names a field the previous step's actual result does not carry, is `will_fail` at publish and in `host.tool_test` with `did_you_mean` naming the corrected path (and `available` listing the fields the result does have). The grammar is the same dotted/indexed `$.a.b[0].c` paths `outputs` accepts elsewhere in this host -- no wildcards, filters, or recursive descent. A step whose mapping resolves to nothing fails the whole call with `error_code: compose_mapping_missing`, naming the path and `failed_step`; add `"on_error": "continue"` to a step to let the remaining steps run anyway (the parent call still ends `error`-adjacent, but names every `failed_steps` index rather than stopping at the first). A chain fired by an event or webhook trigger also reads the delivery as `$.event` (`$.event.body.ref`, `$.event.headers`, `$.event.received_unix`); on any other call `$.event` is `null` and a mapping that reads it fails `compose_mapping_missing`. Publish reports a `$.event` mapping as `unverifiable`, naming the trigger kind.

The chain's own result is its last step's result, unless that step itself failed and every later step tolerated its own failure -- then it's the last step that actually ran. `host.tool_test` on a published chain dry-runs it: each step's resolved arguments are reported and a tool step whose result a later mapping reads is dry-run once (writes rolled back) so that mapping is resolved against its actual result -- `unverifiable` is reserved for a predecessor that cannot be dry-run, and a dry run never reports `pass` while a mapping is `unresolved_path`. Each step after the first lists `prev_fields`, the fields it may read from its predecessor (from the predecessor's declared `outputs` at publish, from its dry-run result in `host.tool_test`). Publish checks every `$.prev.result.<field>` and `$.steps[i].result.<field>` against three sources in order: the predecessor's declared `outputs` (`prev_fields_source: "declared"`), else the keys its completed runs actually returned (`"observed"`, with `runs_sampled`), else `unverifiable` until it has run once; a field none of them has is refused with `compose_mapping_unresolved` (`available`, `did_you_mean`) unless the publish passes `force: true`.

A chain step goes through the same composition primitive `mcphost.call` uses (`kinds::compose_call`): a chain naming itself as one of its own steps is refused with `error_code: compose_self_call`; nesting chains (or python tools calling chains, or chains calling python tools that call further tools) more than 4 levels deep is refused with `error_code: compose_depth_exceeded`; a single top-level call's whole tree is capped at 50 child calls total (`error_code: compose_children_exceeded`).

`host.tool_test` takes `fixtures: {"<step number or tool name>": <recorded result>}` for a chain: a fixtured step executes nothing (no upstream call, no `calls_per_day`), its report entry is `{step, tool, source: "fixture", resolved_args, result}`, and later mappings resolve against it. Every other step reports `source: "dry_run"` (with its `result`) or `source: "skipped"` with a `reason`; the report's `executed`, `fixtured` and `skipped` count them. A fixture that carries none of a step's declared `outputs` is refused with `fixture_shape_mismatch` (`data.expected`, `data.given`); fixtures on a non-chain tool fail `fixtures_not_applicable`, more than 256 KiB in total fail `fixtures_too_large` (`data.limit_bytes`), and a tool name used by two steps needs the step number (`fixture_step_ambiguous`). `host.runs.get(run_id, as_fixtures=true)` exports a finished chain run's child results in exactly that shape (`fixtures`; steps too large for one part are listed under `truncated`).

A step may fan out: add `"for_each": "$.prev.result.items"` (any mapping path to an array) and the step runs once per element, with `$.item` (the element) and `$.index` (its 0-based position) available in that step's `args` -- and nowhere else; `$.item` in a step with no `for_each` is refused at publish with `compose_item_outside_for_each`. At most `min(step.concurrency, plan.concurrent_calls_per_tenant)` items are in flight (`concurrency` is optional, a positive integer). More items than the plan's `chain_fanout_max` (free 20, pro 50; in `host.quickstart` `limits.plan`) is refused before any item runs with `compose_fanout_exceeded` (`data.limit`, `data.actual`); a `for_each` path that is not an array is `compose_mapping_type`, and publish reports one whose predecessor's observed or declared type is not an array as `will_fail` with `expected: "array"`. Each item is one child run (`trigger: "composition"`, `trigger_ref: "<parent_run_id>#<step>:<index>"`) that counts toward the 50-child tree cap. The step's result is `{"items": [r_0, ...], "failed": [{"index", "error_code", "message"}], "ok": n, "failed_count": m}` in input order, a failed item's slot being `null`, and the next step reads it as `$.prev.result.items`. With the default `on_error` the first failed item fails the chain with `compose_fanout_item_failed` (`data.step`, `data.index`, `data.item_error_code`, `data.message`) once in-flight items finish; with `"on_error": "continue"` the chain goes on and names the step in `failed_steps`. An empty array gives `{items: [], failed: [], ok: 0, failed_count: 0}` and the next step runs. An item that cannot start before the chain's deadline fails `deadline_exceeded`.

## Spec fields

<!-- kind-fields:start -->
| field | type | required | meaning |
|---|---|---|---|
| `steps` | array of object | yes | ordered steps, each naming a `tool` and an `args` mapping (`$.input`, `$.prev.result`, `$.steps[i].result` paths or literals) |
| `outputs` | array or object | no | fields a caller can rely on in `result.payload`, optionally with a `$.a.b` path each (every kind) |
| `reads` | array of string | no | tables this tool reads, recorded for lineage and blast-radius reports |
| `ui` | object | no | optional presentation hints for clients that render a tool's result |
<!-- kind-fields:end -->

## Kind ceilings

<!-- kind-ceilings:start -->
| ceiling | free | pro |
|---|---|---|
| `budget.max_child_calls` | 20 | 50 |
| `budget.max_wall_ms` | 60000 | 600000 |
| `chain_fanout_max` | 20 | 50 |
<!-- kind-ceilings:end -->

## Error codes

<!-- kind-errors:start -->
| code | when |
|---|---|
| `compose_self_call` | a tool named itself as one of its own children |
| `compose_depth_exceeded` | composition nested deeper than 4 levels |
| `compose_children_exceeded` | one top-level call's tree exceeded 50 child calls |
| `compose_item_outside_for_each` | `$.item` or `$.index` used in a step with no `for_each` |
| `compose_mapping_type` | a mapping resolved to the wrong type (for example a non-array `for_each`) |
| `compose_mapping_missing` | a mapping path resolved to nothing |
| `compose_fanout_exceeded` | a `for_each` array was longer than the plan's `chain_fanout_max` |
| `compose_fanout_item_failed` | an item of a `for_each` step failed |
| `compose_input_missing` | a `$.input.<name>` mapping named an argument the call did not carry |
| `tool_building` | the tool's dependencies are still building |
| `tool_not_found` | a named tool does not exist for this tenant |
| `tool_run_unsupported` | the tool cannot be run this way (for example a dry run) |
<!-- kind-errors:end -->

## Canonical example

<!-- kind-example:start -->
```json
{
  "steps": [
    {
      "args": {
        "text": "$.input.text"
      },
      "tool": "table_note"
    },
    {
      "args": {
        "rows": "$.prev.result.appended",
        "table": "runs"
      },
      "tool": "host.table.append"
    },
    {
      "args": {
        "note": "$.item"
      },
      "for_each": "$.input.notes",
      "tool": "table_note"
    }
  ]
}
```
<!-- kind-example:end -->

<!-- kind-contract:start -->
## Contract

| Field | Value |
|---|---|
| family `compose` | compose_self_call, compose_depth_exceeded, compose_children_exceeded, compose_item_outside_for_each, compose_mapping_type, compose_mapping_missing, compose_fanout_exceeded, compose_fanout_item_failed, compose_input_missing |
| family `tool_state` | tool_building, tool_not_found, tool_run_unsupported |
| source_line | field `failed_step` |
| bridges | none |
| secret redaction | no |
<!-- kind-contract:end -->
