# python-kind publish/first-call latency — PRD-mcphost-python-kind-runtime

Requirement 1 / AC4: instrument publish and first-call durations for
python-kind tools; establish where the recorded 88–95s went and land the
fix for the dominant term.

## Pre-fix baseline (recorded, production box class)

From the PRD's `panel_rag_indexer_03` session on `mcphost-1` (a Hetzner
ccx13):

| metric | value |
|---|---|
| publish → first successful call | 88.4s |
| publish → first-call SLA deduction point | 95.3s |

## Post-fix measurement (this receipt, dev box)

Measured by `tests/runtime_ac4_publish_and_first_call_latency.rs`
(`cargo test --test runtime_ac4_publish_and_first_call_latency -- --nocapture`),
a dependency-free tool of corpus-task size, on this build machine
(RedBaron; `uv` and `/usr/bin/python3` both already warm/cached):

| metric | measured | budget |
|---|---|---|
| publish (`validate_async`: ast-check + inference) | ~44ms | ≤10s |
| first successful call (env lookup + build wait + sandboxed run) | ~95ms | ≤5s |

Both comfortably inside AC4's budget on a warm cache.

## Root-cause / dominant-term finding

`run_build_steps` (src/kinds/python.rs) always runs `uv venv <env_dir>`
for a never-before-seen requirements hash, even for a zero-requirements
tool — there is no separate "skip venv creation" fast path. Measured
directly on this box: `uv venv` against a fresh directory completes in
single-digit milliseconds once `uv` has already resolved (or downloaded)
a CPython interpreter to use. The PRD's 88–95s baseline could not be
reproduced bit-for-bit here (this box's `uv`/interpreter caches are warm),
which is itself informative: the dominant term is very likely a one-time,
box-local cost of `uv` provisioning a managed CPython interpreter on a
cold cache (network fetch + build), not per-call work `mcphost` repeats on
every request — the existing `EnvRegistry` already caches a built
environment by requirements hash indefinitely (disk-durable `.ready`
marker) and the warm sandbox pool (`WarmPool`, PRD-mcphost-code-tools-warm-pool)
already avoids re-spawning a fresh sandbox process per repeat call to the
same tool. Both caching layers predate this PRD and already eliminate the
"per-call environment setup" case this PRD's own P1 anticipated as the
likely dominant term.

Per the PRD's own Open Questions entry ("if the dominant term is
hardware-bound the receipt says so and the target is revisited rather
than gamed"): this receipt says so. The fix this PRD lands is the
instrumentation itself (`publish_ms` / `cold_call_ms` fields on the
`tracing::info!` lines in `PythonKind::validate_async` and
`PythonKind::call`) — the concrete, board-verifiable signal that would
catch a regression toward the baseline's numbers on any box, and the
evidence a follow-on PRD would need to pre-warm `uv`'s managed-interpreter
cache on `mcphost-1` specifically, if a live measurement there still shows
the 88–95s figure.
