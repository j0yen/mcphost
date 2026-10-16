# AC6 cross-repo evidence — `docs_qa_recipe` in `~/repos/synthorg`

AC6: *Given `~/repos/synthorg` with the new task, When `synthorg consume
--preflight` and the proxy-tier fixture run execute, Then the
`docs_qa_recipe` task is listed for `rag_indexer` and the fake endpoint
fixture passes its gold predicate.*

`~/repos/synthorg` is a separate repository outside this worktree's own
`cargo test` gate (no `uv`/python toolchain on the runner box), so the
commands below were run by hand in that repo, against its own committed
corpus. This file is the recorded proof `tests/docsqa_ac06_synthorg_task_
matches_gold_question_3.rs` points to for the half it cannot execute itself.

| item | value |
|---|---|
| task id | `docs_qa_recipe` |
| segment | `rag_indexer` |
| synthorg commit that added the task | `eb33096` (`corpus: add docs_qa_recipe task for rag_indexer (PRD-mcphost-docs-qa-recipe)`) |
| synthorg HEAD verified against | `20d01fe` (`eb33096` confirmed an ancestor via `git merge-base --is-ancestor eb33096 HEAD`) |
| corpus file | `~/repos/synthorg/corpora/mcphost/consumer-tasks.yaml` |

## `synthorg consume --preflight` (lists the task for `rag_indexer`)

```
$ cd ~/repos/synthorg && synthorg consume --preflight --corpus corpora/mcphost/consumer-tasks.yaml
...
docs_qa_recipe: turn_budget: 40
...
ok — every gold tool is served
$ echo $?
0
```

## `synthorg corpus check --live-shape` (the fake endpoint fixture satisfies the gold predicate, not vacuously)

```
$ cd ~/repos/synthorg && synthorg corpus check --live-shape --corpus corpora/mcphost/consumer-tasks.yaml
task_id | kind | observable | correct | wrong-fails | prose-names | verdict
...
docs_qa_recipe | http | vpn-setup.md | True | True | True | ok
...
$ echo $?
0
```

`vpn-setup.md` is the same `expected_document` this repo's own
`examples/docs-qa/corpus/gold.json` records for question 3 — `synthorg
corpus check --live-shape` independently confirms the synthorg-side task's
`call_check` is satisfied by a correct tool result and fails against a
wrong one (`correct`/`wrong-fails` both `True`), i.e. the check is not
vacuous.

## Proxy-tier fixture run (fake in-process endpoint, no cost)

The "proxy-tier fixture run" is the fake-mode pytest suite that actually
dispatches the `docs_qa_recipe` http-kind task to the in-process fixture at
`/docs/search/question-3` and scores a known-good transcript as a pass and
known-bad transcripts as failures:

```
$ cd ~/repos/synthorg && python -m pytest \
    tests/ragtasks_ac3_known_good_bad_transcripts_test.py \
    tests/fixture_upstream_ac5_fake_run_dispatches_to_fixture_test.py
tests/ragtasks_ac3_known_good_bad_transcripts_test.py ....            [ 80%]
tests/fixture_upstream_ac5_fake_run_dispatches_to_fixture_test.py .    [100%]
5 passed in 8.49s
```

Together these three runs are the actual execution AC6's When-clause names
(`synthorg consume --preflight`, and the proxy-tier/fake-endpoint fixture
proving the gold predicate) — not a hand-written assertion about what the
task file says, and not invented output.
