# satisfaction[rag_indexer] panel record — PRD-mcphost-docs-qa-recipe

AC10: *Given the truth-tier panel run after land, When `synthorg consume`
runs with the pinned panel, Then `rag_indexer` satisfaction is reported with
the new task included (number recorded in the vision, target ≥ 75%).*

This file is this repo's own copy of that record; the record of account is
the vision doc's own `## Satisfaction record` section
(`~/Documents/PRDs/visions/mcphost-data-layer-first-slice.md`, the PRD's
`Vision:`). Both are written by the same recorder,
`examples/docs-qa/record-panel-satisfaction.sh`, which refuses any run whose
corpus did not include this recipe's `docs_qa_recipe` task — a panel number
measured without the task cannot answer this AC.

| item | value |
|---|---|
| segment | `rag_indexer` |
| baseline | 61.8% (2026-09-23 quality baseline, wiki `2026-09-23-mcphost-market-test-plan`) |
| target | ≥ 75% (this PRD's success metric, first truth-tier run after land) |
| corpus task | `docs_qa_recipe` (`~/repos/synthorg/corpora/mcphost/consumer-tasks.yaml`, synthorg commit `eb33096`) |
| pinned panel | `corpora/mcphost/panel-composition.yaml` |
| vision record | `~/Documents/PRDs` commit `05e9541` (the `## Satisfaction record` section this file mirrors) |
| PRD deferral record | `~/Documents/PRDs` commit `7c383c3` (`build-queue/PRD-mcphost-docs-qa-recipe.md` frontmatter: `deferred_acs: [9, 10]` + `mock_justifications`; the repo's own copy at `PRD-mcphost-docs-qa-recipe.md` is what `tests/docsqa_ac10_deferral_is_justified.rs` parses on the runner box) |

## The truth-tier run (deferred — operator-authorized, after land)

The number itself comes from a live, paid, operator-authorized run against
prod, which this build deliberately does not perform. After this branch
lands and `mcphost.dev` serves it, the operator runs:

```
cd ~/repos/synthorg
SYNTHORG_PROD_ENDPOINT=https://mcphost.dev/mcp \
SYNTHORG_TRUTH_BRIEF=<market brief.md> \
  scripts/truth-tier-nightly.sh          # or, directly:
uv run synthorg consume --tier truth --population 21 \
  --composition corpora/mcphost/panel-composition.yaml \
  --endpoint https://mcphost.dev/mcp <brief.md> --out runs/truth-tier-<ts>
```

then records that run's number into both files:

```
cd ~/wintermute/mcphost
examples/docs-qa/record-panel-satisfaction.sh \
  --measure ~/repos/synthorg/runs/truth-tier-<ts>/measure.json \
  --vision ~/Documents/PRDs/visions/mcphost-data-layer-first-slice.md \
  --receipt docs/receipts/docs-qa-panel-satisfaction.md
```

The recorder exits 0 only for a live truth-tier run that met the target; an
under-target number is still recorded (exit 1), because AC10 asks for the
number, and a miss is a number too. The same command with
`MCPHOST_LIVE=1 MCPHOST_PANEL_MEASURE=<that measure.json>` is what
`tests/docsqa_ac10_truth_tier_panel_satisfaction_recorded.rs`'s live half
runs, so the post-land run has a test that fails until it happens.

## What was measured before land (offline, not the market number)

An offline full-corpus panel run over the same pinned composition and the
same corpus — `SYNTHORG_LLM_MODE=fake` (stub personas, stub judge, the
in-process fake MCP endpoint), so it is *not* a market number and the
recorder labels it `offline` and refuses to call it green. What it does
prove, on real run output rather than a hand-written fixture, is that
`docs_qa_recipe` is genuinely wired into the pinned panel and that
`rag_indexer`'s number is computed with it included:

## Satisfaction record — satisfaction[rag_indexer]

- 2026-09-26 offline run runs/offline-docsqa-20260926T053158Z: satisfaction[rag_indexer]=90.0% (target >= 75%, scored 5/5 sessions, docs_qa_recipe included, corpus fingerprint 609b58d058cc)

## Fixtures the test drives, pinned to the runs they came from

`tests/docsqa_ac10_truth_tier_panel_satisfaction_recorded.rs` drives the
recorder with two trimmed copies of real `measure.json` files — same keys the
recorder reads, every value copied unchanged from the run (verified key by key
against the source measures on the build host, 2026-09-27). Synthorg is a
different repository and is not present on this gate's runner box, so the
fixtures cannot be re-derived there; instead each one's byte digest is pinned
here and
`the_receipt_pins_each_fixture_to_the_real_run_it_was_trimmed_from` fails if a
fixture changes without this table changing with it. Editing a *number* in a
fixture is the one way this AC's mechanism proof could quietly become an
invented one, and that is what these digests close.

| fixture | source run (`~/repos/synthorg`) | sha256 (fixture / source measure.json) | why |
|---|---|---|---|
| `examples/docs-qa/fixtures/panel-measure-offline-20260926T053158Z.json` | `runs/offline-docsqa-20260926T053158Z` (`docs_qa_recipe` included, `rag_indexer` 90.0%, `client_version: fake`) | `4b0a66c91b1f07056c0c4062294891726d02fb1512608cfddba00623e0c69999` / `1bef3f26e67304f88caf2b029f55adc39b488380190742f9227d6f84a432aff1` | the recorder's `offline` classification and the record line, on real output |
| `examples/docs-qa/fixtures/panel-measure-truth-20260926T025631Z.json` | `runs/truth-tier-20260926T025631Z` (a real pre-land truth-tier nightly, `rag_indexer` 62.5%, 28 tasks, **no** `docs_qa_recipe`, $4.62) | `97f1c0dc8321ac1fec9511e507389ee4fd5a8d7f28371779dbe47eb670e75f2b` / `8103f1ea5e4920ee6c0ba17cb2e93c1e9bd9f18a825c6dc6035ce54fc34e73f6` | the refusal: a real, live, expensive truth-tier number is still not an answer to AC10 if the task was not in the corpus |

No post-land truth-tier run exists yet, so the measures the target comparison
runs on are **assembled from those two real runs' own values** and nothing
else: the truth nightly's `tier`/`client`/`client_version`/`endpoint_version`/
judge and scorer versions and pinned composition, carrying the offline run's
whole `corpus` object (the only real corpus that contains `docs_qa_recipe`,
fingerprint and all) plus one run's own `satisfaction` and session counts —
the offline run's 90.0% for the green case, the truth nightly's own 62.5% for
the under-target case. Nothing is hand-set, spliced or rounded;
`the_measures_under_test_invent_no_number` fails if a later edit tries to.

## The deferral, as declared

AC10's Then is deferred — **operator-provisioned**. The PRD's own frontmatter
carries `deferred_acs: [9, 10]` and a `mock_justifications` entry for AC10
naming the paid prod run, the credentials this sandbox does not hold
(`ANTHROPIC_API_KEY` / `WM_ANTHROPIC_API_KEY`, `SYNTHORG_PROD_ENDPOINT`,
`SYNTHORG_TRUTH_BRIEF`), the live test file and fn
(`tests/docsqa_ac10_truth_tier_panel_satisfaction_recorded.rs::live_truth_tier_panel_run_meets_the_target`),
and the sentence that the always-on tests prove the branch's mechanism, not
AC10, and are not counted as AC10's proof.
`tests/docsqa_ac10_deferral_is_justified.rs` parses that frontmatter,
`agent/test-map.json` and `agent/intent-card.json` and fails if the three ever
stop agreeing.
