# PRD: mcphost chain run lineage — child runs per step, and a chain that names its own inputs

- Status: queued
- Lane: orch 2026-09-29T21:25:04.934070332+00:00 run=297
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- build_version_bump: minor
- test_prefix: mcphost_chain_run_lineage
- deferred_acs: [13]
- mock_justifications: AC13 -- prod-only, operator-authorized proof ("Given prod mcphost with this PRD deployed and PRD-synthorg-truth-tier-probe-as-tenant landed, When one nightly truth-tier run executes `data-pipeline-builder-daily-pipeline-chain`, ... the recipe scores >= 0.75 (live: nightly `synthorg consume --tier truth` on orch)"). The score can only come from a truth-tier `synthorg consume --tier truth --endpoint https://mcphost.dev/mcp` run: real frontier-model calls billed to the operator's Anthropic key (`ANTHROPIC_API_KEY`, or `WM_ANTHROPIC_API_KEY=` in the box's env file; `synthorg.llm` refuses live mode without it), driven from `~/repos/synthorg` (a different repository, python, not on this gate's runner box) by the operator's nightly timer on host orch (`scripts/truth-tier-nightly.sh`, a RedBaron user timer, 02:30 local), against a prod deployment that does not carry this branch until the operator deploys it -- and it also needs PRD-synthorg-truth-tier-probe-as-tenant (a separate PRD, landed to synthorg master 2026-09-29 per Technical considerations above) so the truth-tier harness can observe this tool's triggers/runs at all. `SYNTHORG_PROD_ENDPOINT` and `SYNTHORG_TRUTH_BRIEF` name the operator's own run config for `scripts/truth-tier-nightly.sh`. Spending an operator's money on a panel run, and deploying to prod, are operator actions, not build-agent actions. The live check is written and ready for the operator's post-land run, gated on MCPHOST_LIVE=1: tests/mcphost_chain_run_lineage_ac13_daily_pipeline_persona_trailer.rs::persona_two_call_sequence_yields_three_done_children, which drives the identical publish/call/call/runs.get sequence against $MCPHOST_URL once deployed. The same file's always-on path runs that sequence for real (the schema-derived `compose_input_missing` refusal on the first call, the same-shape retry succeeding, and `host.runs.list(tool=...)`'s most-recent row inlining exactly 3 done children) against a real server built from this branch. Conjunct 4 -- the >= 0.75 score -- is narrowed rather than deferred whole, because its arithmetic is synthorg's own and is not opaque: `_classify_gold_agreement` scores a session `timeliness * accuracy * helpfulness`, `_recipe_satisfaction` takes the mean over the recipe's scored sessions against `RECIPE_SATISFACTION_TARGET = 0.75`, `helpfulness` is one of three judge labels (`HELPFULNESS_LABEL_VALUES`: blocked 0.0 / partly 0.5 / helped 1.0) and `_accuracy_score` is one of three values, so 0.5 * 1.0 * 1.0 = 0.5 < 0.75 makes `accuracy == 1.0` a strict necessary condition for the bar -- and `accuracy == 1.0` takes the gold tool listed AND `runs.last(tool=daily_pipeline).children.count == 3` observed True, which is exactly what this branch supplies. That scorer is ported and asserted against this branch's own real observation by the same trailer file's `the_recipe_bar_is_unreachable_without_the_children_this_prd_inlines` and `the_ported_scorer_matches_synthorgs_own_arithmetic`, and by the always-on `persona_two_call_sequence_yields_three_done_children` itself, which computes `accuracy` off the live probe (1.0) and off the identical payload with this PRD's `children` stripped back off (0.5, below target at every judge label and every timeliness -- the 0.00 the grounding runs recorded). The probe's own runs-order half lives in synthorg and is landed there: `normalize_probed_runs` + `_emit_host_context_probe(runs_order=...)` in `/home/jsy/repos/synthorg` (master, commit c07c297, committed but not pushed), proven by `~/repos/synthorg/tests/chainlineage_ac13_probe_runs_order_test.py`. What remains operator-provisioned, and all that remains, is the two factors of that product this branch does not own: the nightly judge's own `helped` label and the persona's own wall clock to its first call, from a paid run against a prod deployment. The always-on tests prove the branch's mechanism and conjunct 4's product-side factor, not AC13, and are not counted as AC13's proof; the truth-tier score itself is unrun and unrunnable from this worktree. Requirements 1-9 (schema derivation, compose_input_missing, child run rows, read-back filters, accounting, lifecycle parity) are all proven standalone by AC1-AC12.
- publish: j0yen/public
- Vision: visions/mcphost-chain-run-lineage.md
- build_priority: high
- PM: Joe
- Drafted: 2026-09-29
- Grounding: /home/jsy/Documents/PRDs/evidence/synthorg-truth-tier/truth-tier-20260929T070451Z-failures.md (`step 1 ('fetch_data'): mapping path '$.input.url' resolved to nothing` — data_pipeline_builder_02 on 09-28, workflow_orchestrator_05 on 09-28 and 09-29); transcripts orch:~/repos/synthorg/runs/mcp-host-capabilities-2026-09-09-consume/sessions/{data_pipeline_builder-panel_data_pipeline_builder_02,workflow_orchestrator-panel_workflow_orchestrator_05}.jsonl; /home/jsy/wintermute/mcphost/src/kinds/chain.rs:28-31 (child-run rows explicitly deferred), :188 (`input_schema: {"type":"object"}`), :185-245 (`compose_mapping_missing`), src/kinds/mod.rs:1181 (`COMPOSE_DEPTH_MAX = 4`), :1187 (`COMPOSE_CHILDREN_MAX = 50`), src/runs.rs:185-215 (run row JSON, no `children`/`parent`), migrations/0016_runs_manual.sql, 0018_runs_test_run.sql (pattern for additive run columns; next free number 0057); corpora/mcphost/consumer-tasks.yaml 796-830; dream-seed.md 2026-09-29
- Engineering target: mcphost (`src/kinds/chain.rs`, `src/kinds/mod.rs` `compose_call`, `src/runs.rs`, `src/db.rs`, `migrations/`)

## TL;DR

mcphost's `chain` kind runs N published tools in order with `$.input` / `$.prev` / `$.steps[i]` argument mapping — the "daily pipeline" every data-pipeline persona asks for. Two things make it fail the persona today. First, a chain publishes with `input_schema: {"type":"object"}` (`src/kinds/chain.rs:188`) — it never says which `$.input.<name>` values its steps need, so the agent's first call is `args: {}` and dies at step 1 with `compose_mapping_missing` (4 of 4 chain sessions across 09-28/09-29, both personas; each then republished the chain with the mapping deleted to make it run). Second, a chain run leaves no per-step run rows: `chain.rs:28-31` explicitly deferred "real child-run rows (`host.runs.get` inlining children)" — so "did all three steps finish?" is answerable only by trusting the parent's result blob, and the truth-tier gold `runs.last(tool=daily_pipeline).children.count == 3` can never be observed. Fix: derive the chain's required inputs from its mapping paths at publish (into `input_schema.required` and `host.tool_spec`), refuse a call missing them before any step runs with `compose_input_missing` naming every missing field, and record one child run row per executed step (`trigger: "composition"`, `parent_run_id`) that `host.runs.get` inlines one level deep and `host.runs.list(parent_run_id=)` filters. Child runs inherit the parent's `end_user_subject`, so a per-end-user pipeline stays attributable — the mcphost angle Backenly's single-app backend does not have.

## Problem statement

**Customer Pain Test.** WHO: data-pipeline builders and workflow orchestrators (panel_data_pipeline_builder_03: "runs 200+ daily dbt jobs; any migration risk is existential"; panel_rag_indexer_03: "spends significant cycles on custom orchestration logic"). WHAT: publish three tools plus one chain, run it once, and see that all three steps ran — without babysitting. WHY they can't today: (a) the chain does not tell the caller what to pass, so the first run fails with a mapping error the agent works around by deleting the mapping (which silently changes what the pipeline does); (b) after a run, there is no step-level run record to point at — only the parent's result contains a `steps` trace built by the chain kind itself. CONSEQUENCE: `data-pipeline-builder-daily-pipeline-chain` = 0.00 on 2026-09-29 (n=2 sessions); on 09-28 both sessions hit the same error; every one of the 4 sessions spent 2 extra turns (republish + re-call) and ended with a pipeline whose fetch step no longer takes a URL. The sibling recipe `rag-indexer-reindex-chain-three-steps` has the identical gold shape and is unobservable for the same reason.

**Evidence, cited.**
- Transcript workflow_orchestrator_05, 2026-09-29T07:27:17Z: `host_tool_publish {"name":"daily_pipeline","kind":"chain","spec":{"steps":[{"tool":"fetch_data","args":{"url":"$.input.url"}}, …]}}` → ok; 07:27:22Z `host_tool_call {"name":"daily_pipeline","args":{}}` → `step 1 ('fetch_data'): mapping path '$.input.url' resolved to nothing`; 07:27:27Z republish with `"fetch_data","args":{}`; 07:27:28Z call ok. Same sequence for data_pipeline_builder_02 on 09-28T10:32-10:33Z and workflow_orchestrator_05 on 09-28T10:48Z.
- `src/kinds/chain.rs:188`: the chain's `ToolDescriptor` publishes `input_schema: json!({"type": "object"})` — no `properties`, no `required`, though every `$.input.<name>` path is parsed at publish time (`:57` "parse any `$.` path inside a step's args").
- `src/kinds/chain.rs:223-245`: `compose_mapping_missing` is raised while executing step N — after earlier steps have already run and consumed quota — not before the chain starts.
- `src/kinds/chain.rs:28-31`: "Real child-run rows (`host.runs.get` inlining children, `trigger` values) wait on PRD-mcphost-runs-and-jobs, not built yet -- this kind's own result carries a `steps` trace … as the best available stand-in."
- `src/runs.rs:185-215`: run JSON has `run_id, tool, trigger, trigger_ref, status, …` — no `parent_run_id`, no `children`; `grep -rn children src/runs.rs` → none.
- `src/kinds/mod.rs:1181,1187`: `COMPOSE_DEPTH_MAX = 4`, `COMPOSE_CHILDREN_MAX = 50` — ceilings already enforced per composed tree via `compose_call`, so a child-run count has a hard upper bound today.
- Recipe gold `consumer-tasks.yaml:811`: `runs.last(tool=daily_pipeline).children.count == 3`; `:829` `runs.last(tool=reindex_chain, status=done).children.count == 3`.

**Failure check (one line).** `data-pipeline-builder-daily-pipeline-chain` = 0.00 on 09-29 (n=2) and both sessions on 09-28; every chain session's first call failed `compose_mapping_missing` on `$.input.url`, and the gold's `children.count` has no product data to read.

**Five whys.**
1. Why 0.00? Gold `children.count == 3` is False (no `children` in any run row) and, before that, the first run of the chain failed.
2. Why did the first run fail? The agent called with `args: {}`; step 1 needed `$.input.url`.
3. Why did the agent pass nothing? The published chain's `input_schema` is an empty object schema; `host.tool_spec`/`tools/list` show no required inputs, and `host.tool_test`'s dry run (`chain.rs:197` test-mode branch) reports the steps but not the inputs they will need.
4. Why is the schema empty? The chain kind was scaffolded ("iter-1 scaffold", `chain.rs:7-8`) with mapping parsing for validation only; deriving a schema from `$.input.*` paths was not in that tick's scope, and no test asserts a chain's schema.
5. Why no child runs? Explicitly deferred to a "runs-and-jobs" PRD (`chain.rs:28-31`) that never landed; the parent's `steps` trace was accepted as a stand-in, and nothing downstream (recipes, healthz, `host.runs.list`) forced the issue until the truth-tier gold asked for lineage. Deepest actionable level: (a) publish-time input derivation + call-time pre-check; (b) child run rows written by `compose_call` for every step dispatch, inlined by `host.runs.get`.

## What would have to be true

- Root: an agent can publish a chain, learn its inputs from the spec, run it once, and read three `done` child runs. [testable]
  - Required inputs are derivable at publish. [known: every `$.input.<first segment>` path is parsed at `chain.rs:57-90`; the first segment after `input.` is the argument name]
  - A missing input can be detected before step 1 runs. [known: mapping resolution against `{"input": args}` is pure (`chain.rs:137-190`); evaluate all `$.input.*` paths up front]
  - A child run row can be written per step without breaking the parent's run accounting. [testable: `compose_call` (`kinds/mod.rs:≈1185-1230`) is the single dispatch point; `insert_queued_run` already accepts `trigger`/`trigger_ref` (`hooks.rs` usage) — reuse `trigger="composition"`, `trigger_ref=<parent run id>`]
  - Child rows do not double-count quota or calls. [assumed → testable: `calls` ledger must record the parent once; children are runs, not calls — verify against `callerusage_ac06_no_by_shape_unchanged`]
  - Purge/retention/export handle children. [known patterns: `runs_ac08_tenant_delete_cascades_runs`, `runoverflow_ac06_purge_drops_run_results_bytes_not_user_state`, `mcphost_tenant_data_export_ac01_archive_contents` — extend, don't fork]
- **Weakest link:** double-counting. A chain of 3 steps must still meter as one tool call (the parent) for billing/usage (`metering.rs`, `host.usage`), while showing 1 + 3 run rows. Requirement 6 pins the accounting.

## Goals

1. A chain's `tools/list` entry and `host.tool_spec` declare its required inputs.
2. A chain call missing a required input fails before any step executes, naming every missing field.
3. Every executed chain step is a run row with `parent_run_id`; the parent inlines `children` one level deep.
4. Quota, metering, purge, delete-cascade and export treat child runs correctly.

## Non-goals

- No workflow engine: no branching, `map` steps (P2 in the composition PRD), retries, or fan-out. Depth/children ceilings unchanged (`COMPOSE_DEPTH_MAX = 4`, `COMPOSE_CHILDREN_MAX = 50`).
- No recursion in `children` beyond one level (nested chains list their own children when read directly).
- No change to the `$.` mapping grammar; `outputs` promotion stays deferred as documented in `chain.rs:24-27`.
- No generic DAG/ETL product (Backenly-style "functions"); this stays an agent-published tool primitive.

## User stories

- As **a data-pipeline builder agent**, when I publish `daily_pipeline` with `$.input.url`, I want `host.tool_spec("daily_pipeline")` to show `required: ["url"]`, so that my first call is right.
- As **the same agent**, when I call the chain without `url`, I want `compose_input_missing` listing `["url"]` before step 1 runs, so that I fix the call rather than delete the mapping.
- As **a workflow orchestrator agent**, after one run I want `host.runs.get(run_id).children` to show three rows with `status: "done"`, so that I trust the chain unattended.
- As **an admin/ops agent**, I want `host.runs.list(parent_run_id=X)` and `host.runs.list(trigger="composition")`, so that a failed step is findable without opening the parent's result blob.
- As **a SaaS operator running a per-end-user pipeline**, I want child runs to carry the parent's `end_user_subject`, so that per-user credential resolution and audit (`host.enduser.audit`) stay correct — the per-end-user job Backenly does not do.

## Requirements

**P0**
1. Publish-time derivation: for a `chain` spec, every mapping path of the form `$.input.<name>[...]` contributes `<name>` to the descriptor's `input_schema.properties` (type `{}` unless a literal step arg elsewhere pins it) and to `input_schema.required` (deduplicated, in first-appearance order). A chain with no `$.input.*` paths keeps `{"type":"object"}` exactly as today.
2. `host.tool_spec` / `host.tool_spec_shared` and `tools/list` expose that schema; `host.tool_test` on a chain returns `inputs_required: [...]` alongside its existing step report.
3. Call-time pre-check: before dispatching step 1, resolve every `$.input.*` path against the call's args; if any is missing, fail with `AppError::Structured { code: "compose_input_missing", data: { missing: ["url", …], step: 1, tool: "fetch_data" } }` and message `chain 'daily_pipeline' needs input(s) [url] (used by step 1 'fetch_data')`; zero steps run, zero child rows are written, the parent run row (if the call is run-shaped) is `failed` with `error_class: compose_input_missing`.
4. Child run rows: `compose_call` writes one run row per executed step with `trigger: "composition"`, `trigger_ref: <parent run id>`, new column `parent_run_id` (migration `0057_runs_parent_run_id.sql`, `ALTER TABLE runs ADD COLUMN parent_run_id TEXT` + index `(tenant_id, parent_run_id)`), `tool` = the step's tool, `status` following the step's outcome (`done`/`failed`), `duration_ms`, `error_class`, and `end_user_subject` copied from the parent. Result storage for children follows the existing parts rule (`RUN_PART_BYTES`).
5. Read-back: `host.runs.get(run_id)` on a parent adds `children: [ <run row JSON> … ]` (one level; each child's own `children` is omitted), ordered by step; `host.runs.list` accepts `parent_run_id` and `trigger="composition"`; default `host.runs.list` (no filters) EXCLUDES children (so today's lists stay the same length) unless `include_children=true`.
6. Accounting: the `calls` ledger and `host.usage` count the chain as ONE call (the parent); child rows count toward `runs` retention/purge bytes but not toward `calls`; `jobs_concurrent` admission is checked once for the parent. `busyaudit`/`metering` tests must show identical numbers for a 3-step chain before and after this PRD.
7. Lifecycle parity: tenant delete cascades children (`runs_ac08` pattern); purge of a parent purges its children's results; `host.export` archives children under the parent's run id; `host.runs.cancel(parent)` marks not-yet-started children `cancelled`.

**P1**
8. `host.runs.list(status="failed", trigger="composition")` rows carry `step_no` and `parent_tool` so a failed step is one query away without the parent.
9. `/healthz` gains `runs.composition_children_total` and `runs.composition_parents_failed_input_total` (the `compose_input_missing` count), so the "agents calling chains wrong" rate is visible.

**P2**
10. `host.tool_test` on a chain accepts `args` and reports, per step, which mapping paths resolve and which do not, without running anything (a dry-run of the pre-check with the caller's intended args).

## Success metrics

| Metric | Type | Baseline | Target | Method | Timeframe |
|---|---|---|---|---|---|
| `data-pipeline-builder-daily-pipeline-chain` truth-tier score | primary | 0.00 (09-28, 09-29; n=2/night) | ≥ 0.75 | nightly measure.json after PRD-synthorg-truth-tier-probe-as-tenant lands | first 3 nightly runs after deploy |
| `rag-indexer-reindex-chain-three-steps` truth-tier score | primary | unobservable (gold `children.count`) | ≥ 0.75 | same | same |
| First-call `compose_mapping_missing` per chain session | primary | 4 of 4 sessions | 0 of N (errors become `compose_input_missing` with a correct second call, or no error) | transcripts | same |
| Republish-with-mapping-deleted workaround per chain session | secondary | 4 of 4 | 0 | transcripts (publish after a mapping error with fewer `$.input` paths) | same |
| `calls` count for a 3-step chain | guardrail | 1 | 1 | `callerusage`/`metering` tests | at land |
| Default `host.runs.list` row count for a tenant with one 3-step chain run | guardrail | 1 | 1 (children excluded by default) | integration test | at land |
| Real (non-`harness-`) tenants with ≥ 1 chain tool on prod | guardrail (seed open question) | unknown | counted before any KR | prod admin tool count by kind | before KR |

## Technical considerations

- Live-AC dependency (NOT a build-order gate): this PRD's live recipe AC requires PRD-synthorg-truth-tier-probe-as-tenant's probe fix deployed (landed to synthorg master 2026-09-29) so the truth-tier harness can observe this tool's triggers/runs; the unit/integration ACs are independent and build standalone. Frontmatter Depends-on removed to avoid deadlocking admission on a soft live-only coupling.

- `src/kinds/chain.rs`: `parse_steps` (`:57-90`) already walks every step's `args` for `$.` paths — collect `input.<name>` first segments there into `ChainKind::inputs_required`; `descriptor()` (`:185-190`) builds `input_schema` from it. The call path (`:185-245`) gains a pre-pass over all steps' `$.input.*` paths against `{"input": args}` before the loop.
- `src/kinds/mod.rs` `compose_call` (`≈1185-1230`): it already receives `CallCtx { compose_depth, compose_children, compose_db, compose_kinds, … }` (`:1051-1070`); add `parent_run_id: Option<String>` to `CallCtx`, set by `runs.rs:817-820` (the run-shaped call site) and `handler.rs:4064` (the synchronous `host.tool_call` site — for a synchronous parent with no run row, create the parent row as `done`/`failed` at completion so children have a parent to hang from; this matches how `trigger.fire`'s manual runs already get rows).
- `src/db.rs`: `insert_queued_run` gains `parent_run_id`; `list_runs` gains `parent_run_id` and `include_children` filters; `get_run_children(tenant_id, parent_run_id)`.
- `src/runs.rs`: `run_to_json` adds `parent_run_id` (null for top-level); `get` inlines children via `get_run_children`; `list` default adds `parent_run_id IS NULL`.
- Retention/export: `src/retention.rs` prune and `src/export.rs` archive iterate `runs` by tenant — children are ordinary rows there; add the cascade assertion tests only.
- Positioning (per seed): child runs carry `end_user_subject` (`src/runs.rs:152, 300-312`) — a per-end-user pipeline (each end user's own upstream credentials via `host.vault.*`) stays attributable per step; Backenly's backend runs as one app identity.

## Migration / compatibility

- Migration `0057_runs_parent_run_id.sql` additive, default NULL; older rows are top-level. No shape change to any existing run row except the new nullable `parent_run_id` key (additive, like `manual`/`test` in 0016/0018).
- Chains with no `$.input.*` publish exactly as today. Callers who relied on `compose_mapping_missing` at step N for a missing *input* now get `compose_input_missing` at step 0; `compose_mapping_missing` remains for `$.prev`/`$.steps[i]` paths that resolve to nothing at runtime.
- `host.runs.list` default excludes children — any client that counted runs to infer chain steps (none known) would see fewer rows; `include_children=true` restores.

## Open questions

| # | Question | Owner | Default if unanswered |
|---|---|---|---|
| 1 | For a synchronous `host.tool_call` of a chain (no run row today), create a parent run row always, or only when the chain has ≥ 1 step? | coder at build | always, so lineage is uniform |
| 2 | Should child runs count toward `runs` quotas per plan (if such a quota exists in `plans.rs`)? | Joe | yes, they are rows with results |
| 3 | Does the recipe gold need `status=done` on the parent as well (reindex-chain gold has it, daily-pipeline gold does not)? | Joe (synthorg corpus) | leave the corpus as is |
| 4 | How many prod chains exist among real tenants? | runner on orch | unknown; guardrail metric above |

## Acceptance criteria

1. P0 — Given a chain published with steps `[{fetch_data, args:{url:"$.input.url"}}, {transform, args:{rows:"$.prev.result.rows"}}, {write, args:{rows:"$.prev.result.rows", region:"$.input.region"}}]`, When `host.tool_spec(name)` is read, Then `input_schema.required == ["url","region"]` and `tools/list` shows the same schema.
2. P0 — Given a chain with no `$.input.*` paths, When published, Then its `input_schema` is exactly `{"type":"object"}`.
3. P0 — Given the chain from AC-1, When `host.tool_call(name, args={})` is called, Then it fails `compose_input_missing` with `data.missing == ["url","region"]`, `data.step == 1`, and `host.runs.list(include_children=true)` shows zero `trigger="composition"` rows for this tenant.
4. P0 — Given the chain from AC-1 and args `{url:"…", region:"eu"}`, When run via `host.tool_run` and awaited, Then `host.runs.get(parent).children` has exactly 3 rows in step order, each with `trigger == "composition"`, `parent_run_id == parent`, `status == "done"`, `tool` equal to the step's tool.
5. P0 — Given that same run, When `host.runs.list()` is called with no filters, Then only the parent row appears; When called with `parent_run_id=parent`, Then exactly the 3 children appear; When called with `trigger="composition"`, Then the 3 children appear.
6. P0 — Given a 3-step chain whose step 2 fails with `on_error: "stop"`, When awaited, Then the parent is `failed`, child 1 is `done`, child 2 is `failed` with the step's `error_class`, and no child 3 row exists.
7. P0 — Given a 3-step chain run by an end user with `end_user_subject = "alice"`, When children are read, Then each child's `end_user` equals the parent's.
8. P0 — Given one 3-step chain run, When `host.usage` and the `calls` ledger are read, Then the chain counts as exactly 1 call and the `busyaudit`/`metering` suites pass unchanged.
9. P0 — Given a tenant with one parent and 3 children, When `host.self_offboard` (or admin tenant delete) runs, Then all 4 run rows are gone; and When `host.runs.purge(parent)` runs, Then the children's results are purged too.
10. P0 — Given `host.tool_test` on the chain from AC-1, When called, Then the report includes `inputs_required == ["url","region"]`.
11. P1 — Given a failed child row, When `host.runs.list(status="failed", trigger="composition")` is read, Then the row carries `step_no` and `parent_tool`.
12. P1 — Given two chain runs and one `compose_input_missing` refusal, When `/healthz` is read, Then `runs.composition_children_total == 6` and `runs.composition_parents_failed_input_total == 1`.
13. P0 — Given prod mcphost with this PRD deployed and PRD-synthorg-truth-tier-probe-as-tenant landed, When one nightly truth-tier run executes `data-pipeline-builder-daily-pipeline-chain`, Then the persona's first `host.tool_call` of `daily_pipeline` either succeeds or fails `compose_input_missing` and its second succeeds, the probe observes `runs.last(tool=daily_pipeline).children.count == 3`, and the recipe scores ≥ 0.75 (live: nightly `synthorg consume --tier truth` on orch)
