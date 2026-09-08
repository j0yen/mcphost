# deploy/listings/launch/ — PRD-mcphost-agent-findability P1#6

Three post drafts (Show HN, Anthropic Discord #mcp, r/mcp) generated from
the messaging canvas (`~/repos/PRDs/evidence/mcp-host/gtm/2026-09-08/messaging.md`)
and panel-tested with synthorg's message-test stage, per AC6. No posting —
posting is a human act (PRD non-goal) and this PRD's own open question
("Post the launch kit ... you, or authorize the build to post under
PUBLISH-OK?") is still open, owner Joe.

## How the panel test was run

`synthorg gtm` requires a brief markdown file positionally; AC6's
`--test-variants ... --only-test` invocation was run as:

```
cd ~/repos/synthorg
.venv/bin/synthorg gtm deploy/listings/launch/brief.md \
  --out deploy/listings/launch/panel-results \
  --test-variants deploy/listings/launch/variants.yaml --only-test
```

`brief.md` is this product's front-matter brief (name/one_line/persona/
segments/competitors/seed_queries/fact_terms — the same seven keys
`brief.py` requires); `variants.yaml` is the three candidate headlines,
each avoiding the two claims the gtm Critic and `scripts/copy-claims.sh`
both rejected ("under five minutes" — untested against real p95 latency;
"the only runtime" — uncited superlative) in favor of the measured numbers
`docs/benchmarks/measure-0.26.3-20260908T085001Z.md` already backs.

## Result

Variant 0 won on `wow_share` (see `panel-results/messaging.md` for all
three): switch_share 0.314, wow_share 0.342. That headline is the shared
line across all three post drafts in this directory.

`panel-results/` mirrors the run's exported `INDEX.md`, `messaging.md`,
and `manifest.json` (fake-mode LLM backend by default in this environment;
rerun with `--backend cli` or `--backend api` for a live/record panel).
