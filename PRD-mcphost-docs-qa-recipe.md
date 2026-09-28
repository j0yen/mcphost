# PRD: mcphost-docs-qa-recipe — a docs Q&A tool in a minute, proven on prod and scored by the panel

- Status: queued
- Lane: orch 2026-09-27T19:56:21.513623662+00:00 run=277
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- test_prefix: docsqa
- deferred_acs: [9, 10]
- mock_justifications: AC10 -- prod-only, paid, operator-authorized proof ("Given the truth-tier panel run after land, When `synthorg consume` runs with the pinned panel, Then `rag_indexer` satisfaction is reported with the new task included ... target >= 75%"). The number can only come from a truth-tier `synthorg consume --tier truth --endpoint https://mcphost.dev/mcp` run: ~$5 and ~40 min of real frontier calls, driven from `~/repos/synthorg` (a different repository, python, not on this gate's runner box) against a prod deployment that does not carry this branch until the operator deploys it, and it needs credentials this sandbox does not hold -- the billed Anthropic key (`ANTHROPIC_API_KEY`, or `WM_ANTHROPIC_API_KEY=` in the box's env file; `synthorg.llm` refuses live mode without it), plus `SYNTHORG_PROD_ENDPOINT` and `SYNTHORG_TRUTH_BRIEF` for the operator's own `scripts/truth-tier-nightly.sh`, which is a RedBaron user timer (`deploy/synthorg-truth-tier.timer`, 02:30 local) on a live host. Spending an operator's money on a panel run, and deploying to prod, are operator actions, not build-agent actions. The live check is written and ready for the operator's post-land run, gated on MCPHOST_LIVE=1: tests/docsqa_ac10_truth_tier_panel_satisfaction_recorded.rs::live_truth_tier_panel_run_meets_the_target (MCPHOST_PANEL_MEASURE=<run>/measure.json), which fails until that run's measure.json reports satisfaction[rag_indexer] >= 75% with docs_qa_recipe in the corpus. The same file's always-on tests run the real recorder (`examples/docs-qa/record-panel-satisfaction.sh`) over measures built only from two real synthorg runs' own numbers -- that proves the branch's mechanism (inclusion refusal, truth-vs-offline split, target comparison, the recorded line, idempotency), not AC10, and is not counted as AC10's proof.
  AC9 -- prod-only proof ("Given prod after land, When `docs-qa.sh https://mcphost.dev` runs from carbon with a fresh signup ..."): needs a real signup from carbon against https://mcphost.dev after the operator's deploy, under 60 s, with the receipt committed under `docs/receipts/`; this build has no live operator access and prod does not serve this branch until that deploy. Unchanged by this pass; see agent/test-map.json's AC9 entry.
- publish: j0yen/private
- Vision: visions/mcphost-data-layer-first-slice.md
- Depends-on: PRD-mcphost-docs-semantic-search.md
- Loop: mcphost-buildloop: satisfaction[rag_indexer]
- Grounding: wwhtbt leaf 5 — visions/mcphost-data-layer-first-slice.md; killer-apps recipe method (5/5 shipped); consume corpus has 33 tasks, none needing documents (mcp-host vision loop contract)
- PM: Joe
- Drafted: 2026-09-25
- Engineering target: j0yen/mcphost (`examples/`, `docs/`, `www/llms.txt`, `plugin/`, `scripts/`) plus one consumer task in `~/repos/synthorg/corpora/mcphost/consumer-tasks.yaml` (python, synthorg side)

## TL;DR

A stranger's agent follows one llms.txt section: sign up, put a folder of markdown
with `host.docs.put`, wait for `status.lag_seconds == 0`, publish a python-kind tool
`ask_docs` that calls `host.docs.search` and returns the passages with citations, and
call it — under 60 seconds and inside free-plan quotas. A script in `examples/`
performs the recipe end to end; a Live AC runs it against prod; a synthorg consumer
task lets the `rag_indexer` persona run the same recipe so the persona's satisfaction
is measured, not assumed.

## Problem statement

The document store and search PRDs are primitives; nothing proves that the persona
they were built for can finish the job. The killer-apps vision's method (a recipe a
stranger finishes in a minute, with a Live AC and a panel task) is what made five
recipes shippable (visions/mcphost-killer-apps.md, all five shipped). The `rag_indexer`
persona's 61.8% is the number this fleet exists to move (wiki
2026-09-23-mcphost-market-test-plan), and the loop can only move a number it can
measure: the consume corpus has 33 tasks and none of them needs documents
(mcp-host vision loop contract). Without this PRD the fleet lands two primitives and
no change in the metric.

## Goals

- One documented recipe, one script, one Live AC, one panel task.
- Free-plan feasible: document count, bytes, chunks, calls all inside `free`.
- The tool's answer carries citations (document name, offset) so accuracy is checkable.

## Non-goals

- An LLM answer synthesis step inside the host (the tool returns passages; the
  calling agent composes the answer).
- PDF corpora.
- Promotion of the recipe (the 09-09 hold stands; docs only).

## User stories

- **Stranger's agent:** reads the llms.txt "Docs Q&A in a minute" section and finishes
  it without any other page.
- **Panel persona (rag_indexer):** runs the task; gold is a top-5 hit on a seeded
  corpus question; timeliness is signup-to-answer under 60 s.
- **Operator:** `examples/docs-qa.sh <endpoint>` is the Live AC; its receipt records
  signup, put, index-ready, publish, call timings.
- **Tool author:** `examples/docs-qa/ask_docs.py` is a 30-line python-kind tool they
  can copy.

## Requirements

**P0**
1. `examples/docs-qa/` contains `ask_docs.py` (python kind: `search` then format
   passages with `name:offset` citations), `corpus/` (8 markdown files, ~40 KiB total,
   with 6 gold questions in `gold.json`), and `docs-qa.sh <endpoint>` that signs up,
   puts the corpus, polls `host.docs.status` until `lag_seconds == 0` (timeout 90 s),
   publishes `ask_docs`, asks the 6 gold questions, and writes a receipt JSON with
   per-step timings and hit results; exit 0 only when ≥ 5 of 6 hit.
2. `www/llms.txt` and `docs/agent-quickstart.md` gain the "Docs Q&A in a minute"
   section: the exact tool calls in order, the quota the recipe uses (8 docs, ~40 KiB,
   ~60 chunks, 1 tool, 7 calls), and the citation format.
3. The Claude Code plugin (`plugin/`) gains a `docs-qa` snippet/command that runs the
   recipe against a configured endpoint.
4. A repo test runs `docs-qa.sh` against the in-process test host and asserts the
   receipt (hit ≥ 5/6, index-ready under 30 s).
5. Synthorg side (python, `~/repos/synthorg`): one consumer task `docs_qa_recipe` in
   `corpora/mcphost/consumer-tasks.yaml` for the `rag_indexer` segment with gold
   `{tool: "ask_docs", predicate: top5 hit on question 3}`; committed with a passing
   `synthorg consume --preflight` fixture run.

**P1**
6. `docs-qa.sh --embeddings <endpoint> <model> <secret-name>` variant that configures
   the provider first and reports both modes' hit rates in one receipt.

**P2**
7. `host.quickstart` output mentions the recipe when the tenant has zero documents.

Non-functional: the recipe's wall time from signup to first answer is under 60 s on
prod (timeliness for the persona); the corpus and script are under 100 KiB in the
repo.

## Success metrics

| metric | baseline | target | method | timeframe |
|---|---|---|---|---|
| satisfaction[rag_indexer] | 61.8% | ≥ 75% | synthorg consume with the new task, same panel pin | first truth-tier run after land |
| recipe wall time on prod | n/a | < 60 s | Live AC receipt | at land |
| guardrail: free-plan quota use | n/a | ≤ 10% of docs_max, ≤ 1% of calls_per_day | receipt | at land |

## Technical considerations

- The Live AC follows the killer-apps convention: the script runs against
  `https://mcphost.dev` with a throwaway signup tagged `source: "recipe-docs-qa"` so
  attribution can exclude it.
- The synthorg task references the same corpus by path; the harness copies it into
  the session's workspace (consume already supports file fixtures per the loop
  contract's corpus format).
- The python-kind tool uses the state helper's loopback endpoint for `search` so it
  does not spend a public tool call per query.

## Migration / compatibility

Docs, examples, plugin snippet, one test, one synthorg task. No host code change.

## Open questions

| question | owner | due |
|---|---|---|
| Whether the recipe signup on prod is deleted after the Live AC (`host.self_offboard`) — drafted yes | Joe | at build |

## Acceptance criteria

1. P0 — Given the in-process test host, When `examples/docs-qa/docs-qa.sh <url>` runs, Then it exits 0, the receipt shows `index_ready_secs < 30`, `hits ≥ 5`, and every answer carries a `name:offset` citation.
2. P0 — Given the receipt, When its quota section is read, Then documents ≤ 8, chunks ≤ 100, tool calls ≤ 10 — inside free-plan knobs.
3. P0 — Given `www/llms.txt`, When the "Docs Q&A in a minute" section is followed literally by the test (each call parsed from the doc and executed), Then the sequence succeeds — the doc and the script agree.
4. P0 — Given a corpus document is updated to change the answer to question 3, When the script re-asks after `lag_seconds == 0`, Then the new passage is returned.
5. P0 — Given the plugin `docs-qa` command, When invoked against the test host, Then it runs the same script and prints the receipt path.
6. P0 — Given `~/repos/synthorg` with the new task, When `synthorg consume --preflight` and the proxy-tier fixture run execute, Then the `docs_qa_recipe` task is listed for `rag_indexer` and the fake endpoint fixture passes its gold predicate.
7. P1 — Given `--embeddings` with a test provider, When the script runs, Then the receipt reports hit rates for both lexical and embeddings modes.
8. P0 — Given a search returning zero passages, When `ask_docs` runs, Then it returns a structured "no passages found" result, not an error.
9. P0 — Given prod after land, When `docs-qa.sh https://mcphost.dev` runs from carbon with a fresh signup, Then it exits 0 in under 60 s and the receipt is committed under `docs/receipts/` (Live; evidence: `mcphost-1: healthz version after deploy plus the call transcript this AC names, saved under docs/receipts/<slug>.md`)
10. P0 — Given the truth-tier panel run after land, When `synthorg consume` runs with the pinned panel, Then `rag_indexer` satisfaction is reported with the new task included (number recorded in the vision, target ≥ 75%).
