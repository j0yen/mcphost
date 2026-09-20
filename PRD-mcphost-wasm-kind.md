# PRD: mcphost-wasm-kind — a third kind: compiled components with instant cold starts

- Status: queued
- Lane: orch 2026-09-19T21:26:08.421052456+00:00 run=17
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- build_priority: normal
- build_version_bump: minor
- publish: j0yen/private
- test_prefix: wasmkind
- Vision: visions/mcp-host.md
- Grounding: opportunity — wwhtbt leaf 2 caveat in visions/mcp-host.md (cycle-53 OS-dependent isolation); held component pulled forward by operator 2026-09-12
- Loop: mcphost-buildloop: satisfaction
- PM: Joe Yen
- Drafted: 2026-09-12
- Engineering target: extend `~/wintermute/mcphost` — a new `wasm` kind beside `echo`/`http`/`python` (`src/kinds/`), Wasmtime as a library, publish validation, plans/limits, docs/llms.txt

## TL;DR

The vision has held a Wasm runtime as the python kind's principled successor since the
first fleet (2026-09-02) and Joe pulled it forward on 2026-09-12. This PRD ships the
narrow slice: a `wasm` kind whose spec carries a compiled WebAssembly component; calls
execute under Wasmtime with fuel, memory, and time limits enforced by the runtime
itself rather than an external sandbox; results follow the same envelope contract as
every other kind. The python kind stays the default authoring path — this kind exists
for tools that need millisecond cold starts and isolation that does not depend on the
host box's userns/AppArmor policy, which has already broken the python kind in
production once (the bwrap incident, cycle 53).

## Problem statement

The python kind's isolation is a property of the host's OS configuration, not of
mcphost: on 2026-09-04, 45 of 45 python publishes failed on the hub because Ubuntu's
`apparmor_restrict_unprivileged_userns` broke bwrap, an entire capability dead in
production while `/healthz` claimed the sandbox was fine (vision, cycle 53). The fix
was an AppArmor profile the deploy tool must now carry to every box. Cold starts are
the second cost: per-tenant virtualenvs mean N× disk and build time (stress audit,
2026-09-09) and the first call after publish paid a warm-up the first-call-reliability
PRD had to paper over. A compiled component with an embedded runtime removes both
classes: isolation travels with the binary, and instantiation is milliseconds. The
production runtime landscape supports it (WASI 0.3 with async I/O since February
2026; Wasmtime is a Rust library — vision, Components/held section).

## Goals

- An agent can publish a compiled Wasm component and call it like any tool, with the
  same envelope, logs, `tool_test`, metering, and quota surfaces.
- Resource limits (fuel, memory, wall time) are enforced by the runtime per call and
  produce the same structured errors the python kind's caps produce.
- Isolation holds on a box with no bwrap, no userns, and no AppArmor profile.

## Non-goals

- No server-side compilation of source to Wasm (the agent brings a component; a
  compile service is its own product decision).
- No Python-in-Wasm (component tooling for Python is still immature — the reason this
  was held; the python kind remains for Python source).
- No change to the python kind or its sandbox.
- No WASI sockets/HTTP from inside the component in this slice (pure
  compute + stdin/stdout-style I/O through the call arguments and result; outbound
  calls stay the http kind's job).

## User stories

1. **Agent with a hot path.** When my tool is called many times and the 30-second
   python budget or venv warm-up hurts, I want to publish a compiled component, so
   calls start in milliseconds under hard caps.
2. **Operator on a locked-down box.** When the OS forbids unprivileged userns, I want
   wasm tools to keep working, so one distro policy change can never kill a whole
   kind again.
3. **Agent testing before publish.** When I run `host.tool_test` on a wasm spec, I
   want the same dry-run surface every kind has, so my workflow does not change per
   kind.

## Requirements

**P0**
1. Spec: `kind: wasm` with the component binary supplied at publish (base64 field or
   the existing source-upload path, whichever the python kind's transport already
   uses), size-capped per plan; validation rejects a non-component binary and
   oversize payloads with structured errors naming the bound.
2. Execution: each call instantiates the component under Wasmtime with per-call fuel,
   a memory cap, and the plan's wall-time budget; exceeding any produces a structured
   error (`tool_oom`/`tool_timeout`-class codes consistent with the python kind's).
3. Envelope: results land as `result.payload` per the envelope contract
   (PRD-mcphost-result-envelope-contract lineage), including declared-output
   promotion identical to the python kind's.
4. Surfaces: `host.tool_test` dry-runs a wasm spec; `host.tool_logs` carries the
   component's stderr/trap messages; metering records calls exactly as other kinds.
5. No OS dependency: the wasm path never invokes bwrap, unshare, or any external
   sandbox binary; a test proves a wasm call succeeds with the sandbox mechanism
   reported unavailable.
6. Docs: descriptor text, README, quickstart, and llms.txt document the kind, its
   limits, and when to choose it over python.

**P1**
7. Module cache: compiled artifacts are cached per tool version so repeat calls skip
   compilation; cache invalidates on republish (the warm-pool invalidation rule).
8. `/healthz` reports the wasm runtime's availability and version beside
   `sandbox_mechanism`.

**P2**
9. A corpus-task proposal (via the established `promote-usecase` proposal path, not
   auto-append) exercising publish-and-call of a wasm tool, so the harness can see
   the kind once a task is adopted.

## Success metrics

| metric | baseline | target | method | timeframe |
|---|---|---|---|---|
| Cold call latency (publish→first call ready) | python: venv build, seconds–minutes | wasm: < 100 ms instantiation in test | wasmkind AC | on ship |
| Kind survives no-userns box | python: dead (cycle 53) | wasm call green with sandbox unavailable | wasmkind AC | on ship |
| Envelope parity | n/a | same promotion tests pass for wasm | shared envelope tests | on ship |

## Technical considerations

- Wasmtime as a crate dependency; pin a version and record it in `/healthz` (P1-8).
  Fuel + `StoreLimits` for memory; epoch or fuel-based interruption for time.
- Reuse the kinds registry and dispatch (`src/kinds/mod.rs`) — this is the third
  implementation of an existing trait surface, not new plumbing.
- Component-model validation at publish (reject core-module-only binaries with a
  message naming component-model as the requirement).
- Binary size affects the DB row (spec storage): store the component beside specs the
  way python source is stored today; if python source lives in the spec JSON, follow
  it; caps per plan.

## Migration / compatibility

Purely additive: a new kind value; no schema change to existing rows beyond whatever
additive migration stores components. Older clients never send `kind: wasm`.

## Open questions

| question | owner | due |
|---|---|---|
| Component size cap per plan (free vs pro) | Joe | at build |
| Whether WASI HTTP joins later or outbound stays http-kind-only permanently | Joe | after first real wasm tool |

## Acceptance criteria

1. P0 — Given a valid component that echoes its arguments, When published as `kind: wasm` and called, Then the call succeeds with the output under `result.payload` and a metering row exists.
2. P0 — Given a core module that is not a component, and a component over the size cap, When published, Then each is refused with a structured error naming the requirement or bound.
3. P0 — Given a component that loops forever, and one that allocates past the memory cap, When called, Then each terminates within the budget with the corresponding structured error code, and the process serves the next call normally.
4. P0 — Given the sandbox mechanism is reported unavailable (bwrap absent), When a wasm tool is called, Then it succeeds, and a python call on the same host still fails with `sandbox_unavailable` — proving independence.
5. P0 — Given a wasm spec, When `host.tool_test` runs, Then the dry-run executes the component with the rendered arguments and returns the same test surface shape other kinds return.
6. P0 — Given a component that traps, When called, Then the trap message is in `host.tool_logs` and the call error is structured, not a raw panic.
7. P0 — Given the build completes, When llms.txt and the kind descriptors are read, Then the wasm kind, its limits, and the choose-wasm-vs-python sentence are present.
8. P1 — Given a tool called twice, When the second call runs, Then instantiation reuses the cached compiled artifact (observable via timing or a counter), and a republish invalidates it.
9. P1 — Given `/healthz` after ship, When read by an admin, Then it reports wasm runtime availability and version.
10. P0 — Given declared outputs on a wasm spec, When the tool returns a matching structure, Then promotion into `result.payload.<field>` behaves identically to the python kind on the same fixture.
