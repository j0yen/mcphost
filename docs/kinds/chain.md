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

Each step names a `tool` (unqualified, same tenant) and an `args` object mapping that step's own call arguments -- each value is either a literal JSON value or (a string starting with `"$."`) a path resolved against `{"input": <the chain's own call args>, "prev": {"result": <the previous step's result>} (null on the first step), "steps": [{"result": <step's result>}, ...] (every earlier step, by 0-based index)}`. `$.prev` has exactly one key, `result`; `$.steps[i]` has exactly one key, `result`. So the field `action` of the previous step's result is `$.prev.result.action`, never `$.prev.action`; a mapping that omits `result`, or names a field the previous step's actual result does not carry, is `will_fail` at publish and in `host.tool_test` with `did_you_mean` naming the corrected path (and `available` listing the fields the result does have). The grammar is the same dotted/indexed `$.a.b[0].c` paths `outputs` accepts elsewhere in this host -- no wildcards, filters, or recursive descent. A step whose mapping resolves to nothing fails the whole call with `error_code: compose_mapping_missing`, naming the path and `failed_step`; add `"on_error": "continue"` to a step to let the remaining steps run anyway (the parent call still ends `error`-adjacent, but names every `failed_steps` index rather than stopping at the first).

The chain's own result is its last step's result, unless that step itself failed and every later step tolerated its own failure -- then it's the last step that actually ran. `host.tool_test` on a published chain dry-runs it: each step's resolved arguments are reported and a tool step whose result a later mapping reads is dry-run once (writes rolled back) so that mapping is resolved against its actual result -- `unverifiable` is reserved for a predecessor that cannot be dry-run, and a dry run never reports `pass` while a mapping is `unresolved_path`. Each step after the first lists `prev_fields`, the fields it may read from its predecessor (from the predecessor's declared `outputs` at publish, from its dry-run result in `host.tool_test`).

A chain step goes through the same composition primitive `mcphost.call` uses (`kinds::compose_call`): a chain naming itself as one of its own steps is refused with `error_code: compose_self_call`; nesting chains (or python tools calling chains, or chains calling python tools that call further tools) more than 4 levels deep is refused with `error_code: compose_depth_exceeded`; a single top-level call's whole tree is capped at 50 child calls total (`error_code: compose_children_exceeded`).

`host.tool_test` takes `fixtures: {"<step number or tool name>": <recorded result>}` for a chain: a fixtured step executes nothing (no upstream call, no `calls_per_day`), its report entry is `{step, tool, source: "fixture", resolved_args, result}`, and later mappings resolve against it. Every other step reports `source: "dry_run"` (with its `result`) or `source: "skipped"` with a `reason`; the report's `executed`, `fixtured` and `skipped` count them. A fixture that carries none of a step's declared `outputs` is refused with `fixture_shape_mismatch` (`data.expected`, `data.given`); fixtures on a non-chain tool fail `fixtures_not_applicable`, more than 256 KiB in total fail `fixtures_too_large` (`data.limit_bytes`), and a tool name used by two steps needs the step number (`fixture_step_ambiguous`). `host.runs.get(run_id, as_fixtures=true)` exports a finished chain run's child results in exactly that shape (`fixtures`; steps too large for one part are listed under `truncated`).

## Retrying a step

A step may declare `"retry": {"max": 1..5, "backoff_ms": 0..30000, "on": [<family>...]}`. `max` is the number of retries after the first attempt (so up to `max + 1` attempts); the wait before retry *n* is `backoff_ms × 2^(n-1)` with ±10 % jitter, or the upstream's `retry_after_s` when that is larger. A step's `retry.on` accepts only these error families: `upstream_auth`, `upstream_rate_limited`, `upstream_client_error`, `upstream_error`, `host_not_allowed`, `tool_timeout`. An unknown member is refused at publish with `error_code: retry_on_unknown` and `data: {given, accepted}`. Codes outside that set (`args_invalid`, `tool_exception`, `compose_*`) are never retried, even if a forced publish lists them.

A retrying step's trace entry carries `attempts: [{attempt, waited_ms, error_code?}]`, and its single child run row carries `attempt` = total attempts. When a wait would pass the chain's remaining deadline, the step stops with the last error and `data.retry_deadline_exceeded: true` (no wait happens); after `max` retries it fails with the last error and `data: {attempts, retry_exhausted: true}`. `host.tool_test` reports each step's `retry` policy and performs no retry.
