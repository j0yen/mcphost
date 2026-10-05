# PRD: mcphost event trigger self-test — `host.trigger.test` signs for you, and the delivery's result is readable in one call

- Status: queued
- Lane: orch 2026-09-29T21:22:17.520540645+00:00 run=298
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- build_version_bump: minor
- publish: j0yen/public
- Vision: visions/mcphost-event-trigger-self-test.md
- build_priority: high
- PM: Joe
- Drafted: 2026-09-29
- Grounding: /home/jsy/Documents/PRDs/evidence/synthorg-truth-tier/truth-tier-20260929T070451Z-failures.md (integration_specialist_02: `signature in 'X-Hub-Signature-256' did not match`, 2026-09-29T07:38:09Z; same text on 2026-09-28T03:59:03Z in workflow_orchestrator_05; `host_quickstart: kind 'event' is not registered` on both nights); transcript orch:~/repos/synthorg/runs/mcp-host-capabilities-2026-09-09-consume/sessions/integration_specialist-panel_integration_specialist_02.jsonl; /home/jsy/wintermute/mcphost/src/hooks.rs (`test` ≈930-990, body re-serialized at :954, `verify_event` :313-356, `build_run_args`), src/webhooks.rs:14-43 (`test_webhook_trigger` self-signs), src/runs.rs:205 (`"result": Value::Null` in list rows), src/runs.rs:38 (`RUN_PART_BYTES = 256 KiB`), src/handler.rs:1668-1728 (`host.trigger.set` schema), src/plans.rs:266-272 / 341-347 (event quotas); corpora/mcphost/consumer-tasks.yaml 736-792; dream-seed.md 2026-09-29 (Backenly context)
- deferred_acs: [11]
- mock_justifications: AC11 -- prod-only, paid, operator-authorized proof ("Given prod mcphost with this PRD deployed and PRD-synthorg-truth-tier-probe-as-tenant landed, When one nightly truth-tier run executes `integration-specialist-github-push-webhook-handler`, Then the persona reaches a `trigger=event` run with `status=done` using <= 1 `host.trigger.test` call and the recipe scores >= 0.75"). The number can only come from a nightly `synthorg consume --tier truth --endpoint https://mcphost.dev/mcp` run on orch: real frontier calls billed to the operator's Anthropic key (`ANTHROPIC_API_KEY`, or `WM_ANTHROPIC_API_KEY=` in the box's env file; `synthorg.llm` refuses live mode without it), plus `SYNTHORG_PROD_ENDPOINT` for the operator's own `scripts/truth-tier-nightly.sh`, a user timer (`deploy/synthorg-truth-tier.timer`) on a live host, driven from `~/repos/synthorg` -- a different repository, python, not present on this gate's runner box -- against a prod deployment that does not carry this branch until the operator deploys it. Deploying to prod and spending an operator's money on a panel run are operator actions, not build-agent actions. The live check is written and ready for the operator's post-deploy nightly, gated on MCPHOST_LIVE=1: tests/mcphost_event_trigger_self_test_ac11_truth_tier_persona_reaches_event_run.rs::live_truth_tier_persona_run_meets_the_target (MCPHOST_TRUTH_LEDGER=<run>/ledger.jsonl, MCPHOST_TRUTH_TRANSCRIPT=<session>.jsonl), which fails until that nightly's own ledger row reports satisfaction >= 0.75 with <= 1 `host.trigger.test` call and a done `trigger=event` run meeting the corpus gold. The same file's always-on tests replay the corpus task's own persona path end to end against a real in-process mcphost (one self-test, no HMAC computed by the caller, the gold read back through `host.runs.list(include_result=true)`) and run the live check's own bar over the committed, verbatim artifacts of the real 2026-09-29T20:06:55Z pre-feature nightly -- that proves the branch's own half (the path exists now, and the bar rejects the night that lacked it), not AC11, and is not counted as AC11's proof.
- Engineering target: mcphost (`src/hooks.rs`, `src/runs.rs`, `src/handler.rs`, `src/llms_txt.rs`)

## TL;DR

mcphost already has inbound triggers: `host.trigger.set(kind="event")` mints a public `POST /hooks/<ns>/<tool>` URL with HMAC-SHA256/SHA1/token/Stripe-style verification (`src/hooks.rs`), and `kind="webhook"` mints an inbox URL whose self-test signs a synthetic delivery for you (`src/webhooks.rs:14-43`). The `event` kind's self-test does not: `host.trigger.test(id, body, headers)` re-serializes the caller's `body` with `serde_json::to_vec` (`src/hooks.rs:954`) and then demands a caller-computed HMAC over those exact bytes — bytes the caller never sees. An agent with no shell (the truth-tier persona; any agent whose client disables Bash) can only guess the hex. It guessed right on 09-28 (4 test deliveries returned `commit_count`) and wrong on 09-29 (`signature in 'X-Hub-Signature-256' did not match`), and the recipe `integration-specialist-github-push-webhook-handler` sits at 0.00 both nights. Fix, in three parts: (1) `host.trigger.test` for `kind="event"` self-signs when no signature header is supplied and returns the exact bytes+headers it signed so the agent can replay them at the public URL; (2) `verify: "github"` / `"stripe"` presets for `kind="event"` so the agent stops hand-assembling `{scheme, header, prefix}`; (3) `host.runs.list(include_result=true)` inlines a terminal run's result when it fits one part, so "what did my hook run return" is one call. Differentiation vs Backenly stays intact: the fired run executes under mcphost's tenant/end-user model (`end_user_subject` on runs, `src/runs.rs:300-312`) — Backenly has no per-end-user credential passthrough at all.

## Problem statement

**Customer Pain Test.** WHO: an integration-specialist agent (segment `integration_specialist`, panel_integration_specialist_03: "handling credential rotation, data transformation, and webhook deployments") that must prove a GitHub-style push hook works end to end before pointing a real repo at it. WHAT: one call that says "a signed delivery to your URL runs your tool and here is what it returned." WHY they can't today: mcphost's event self-test verifies a signature the caller must compute over bytes mcphost produces after parsing — with no shell, the agent either publishes a helper tool to compute HMAC (3-6 extra turns) or guesses. CONSEQUENCE: recipe at 0.00 on 2026-09-28 and 2026-09-29 (n=2 sessions, 2 personas); 35 of 80 turns spent on 09-29; the 09-28 session needed four `host.trigger.test` attempts (one `args_invalid`, two successes with hand-rolled signatures, one with the wrong count) before the persona stopped. The discover panel's demo bar — "paste an agent definition, one tool call, and there's a live webhook URL" (workflow_orchestrator_03) — is met for the URL and missed for the proof.

**Evidence, cited.**
- `src/hooks.rs:954`: `let body_bytes = serde_json::to_vec(&body_value).unwrap_or_default();` then `:963 verify_event(&verify, secret.as_deref(), &body_bytes, &headers)?;`. `serde_json` is built without `preserve_order` (`Cargo.toml:26`), so key order and whitespace of the agent's original JSON are not what gets verified.
- `src/webhooks.rs:14-43`: `test_webhook_trigger` builds the body, computes `x-mcphost-signature: sha256=<hmac>` itself (:30-31) or a Stripe `t=,v1=` header (:35-37), and returns `{row_id, run_id, test: true}`. The two kinds disagree on who signs.
- Transcript 2026-09-29T07:36-07:38Z: `host_secret_set {"name":"hook-secret-1"}` → `host_trigger_set {"kind":"event","verify":{"scheme":"hmac-sha256","header":"X-Hub-Signature-256","secret":"hook-secret-1","prefix":"sha256="}}` → `{"enabled":true,"url":"https://mcphost.dev/h…"}` → `host_trigger_test {body: {...commits: [c1,c2,c3]...}, headers: {"X-Hub-Signature-256":"sha256=…"}}` → `signature in 'X-Hub-Signature-256' did not match`.
- Transcript 2026-09-28T10:57Z and 2026-09-29T07:36Z: `host_quickstart(kind="event")` → `kind 'event' is not registered; registered kinds: ["chain","echo","http","python","wasm"]` — the agent's first guess is that triggers are a tool kind, because `host.quickstart` and llms.txt never mention triggers (`grep -n trigger src/llms_txt.rs` → no matches).
- `src/runs.rs:205`: list rows always carry `"result": null`; only `host.runs.get`/`wait` inline part 0 (:224-262). A poller that wants the hook run's output must issue N+1 calls.
- Recipe gold (`consumer-tasks.yaml:768`): `runs.last(trigger=event).result.payload.commits == 3` — the scorer reads `result` from `host.runs.list` (synthorg probe), which is null today even for a 1 KB result.
- Quotas already exist and are not the blocker: free `event_triggers_max = 3`, 30 events/min (`src/plans.rs:271-272`); `signature_invalid` → 401 (`src/hooks.rs` `status_for_hook_error`).

**Failure check (one line).** `integration-specialist-github-push-webhook-handler` = 0.00 on 09-28 and 09-29; last error both nights `signature in 'X-Hub-Signature-256' did not match` from `host.trigger.test`.

**Five whys.**
1. Why 0.00? The self-test rejected the persona's delivery, so no `trigger=event` run with the expected result exists.
2. Why rejected? The `X-Hub-Signature-256` the persona supplied did not equal HMAC(secret, `serde_json::to_vec(parsed body)`).
3. Why did they differ? The persona signed its own JSON text (or guessed); mcphost verifies over a canonical re-serialization it never returns to the caller (`src/hooks.rs:954`).
4. Why does the event kind require a caller-supplied signature at all? `hooks.rs` was built to "drive the exact same verify-then-enqueue path `handle_hook` does" (doc comment above `test`) so real-sender rejections are reproducible; the later `webhook` kind (`webhooks.rs`) chose self-signing instead, and the two were never reconciled.
5. Why was that not caught before a persona hit it? `tests/wake_ac7_trigger_test_synthetic_envelope.rs` and the event tests exercise `test` from Rust, where computing the HMAC over `serde_json::to_vec` is trivial; no test models a caller that cannot run code. Deepest actionable level: make the event self-test self-signing by default (parity with `webhooks.rs:14-43`) and return the signed bytes, keeping the strict caller-signed path available when a signature header is present.

## What would have to be true

- Root: an agent with only MCP tool calls can prove its event hook end to end in ≤ 4 calls (`secret_set`, `trigger.set`, `trigger.test`, `runs.wait`). [testable]
  - The self-test can produce a valid signature without the caller. [known: the secret is resolvable at test time — `resolve_secret(state, tenant.id, &verify)` at `src/hooks.rs:≈960`; `webhooks.rs:30` already does this for the sibling kind]
  - The agent can later replay the same delivery at the public URL from another system. [testable: return `signed.body` (exact string) and `signed.headers`]
  - The run's result is readable where the agent (and the scorer) looks. [known false today: `src/runs.rs:205`; testable after `include_result`]
  - Presets do not weaken verification. [known: GitHub = HMAC-SHA256 over raw body, header `X-Hub-Signature-256`, prefix `sha256=`; `verify_event` already implements this scheme at `src/hooks.rs:331-353`]
  - The strict path still exists for callers who *can* sign. [design: presence of the configured header switches to strict]
- **Weakest link:** ambiguity between "no header supplied → self-sign" and "header supplied but wrong → reject". A caller who sends an empty-string header must be rejected, not auto-signed, or a broken sender passes its own test. Requirement 1 pins: self-sign only when the configured header is absent from `headers`.

## Goals

1. Event-trigger self-test parity with the webhook kind: mcphost signs when the caller does not.
2. The self-test's response is a replayable artifact (exact body bytes + headers).
3. Presets for the two senders the corpus and panel name (GitHub, Stripe) on `kind="event"`.
4. A terminal run's result is readable from `host.runs.list` when it fits one part.
5. Triggers are discoverable from `host.quickstart`/llms.txt in one sentence.

## Non-goals

- No change to the public `POST /hooks/...` accept path, verification schemes, dedupe, or quotas.
- No generic BaaS surface (no "functions", no REST scaffolding) — this is the proof step for an agent-published tool's inbound trigger, not a Backenly clone.
- No new trigger kinds; no change to `kind="webhook"` (already self-signing) or `kind="message"`.
- No result inlining for runs whose result exceeds one part (`RUN_PART_BYTES`); those keep `result: null, result_ref`.

## User stories

- As **an integration-specialist agent** with no shell, I want `host.trigger.test(id, body)` to sign the delivery for me and tell me the headers it used, so that I can prove the hook and hand the same headers to the real sender's docs.
- As **a workflow-orchestrator agent** (panel_workflow_orchestrator_03), I want `verify: "github"` on `trigger.set`, so that I do not hand-assemble scheme/header/prefix and mistype one.
- As **a RAG-indexer agent** (rag_indexer_01) wiring `reindex_on_push`, I want to read the fired run's result from the same `host.runs.list(trigger="event")` I already poll, so that confirmation is one call.
- As **a SaaS operator whose end users each connect their own GitHub** (mcphost's per-end-user wedge), I want the fired run to carry the same `end_user_subject` semantics as any other run, so that per-user credentials from `host.vault.*` resolve the same way — something Backenly's single-app backend does not model.
- As **a first-run agent** reading `host.quickstart`, I want one line that says triggers are `host.trigger.set(kind=...)`, not a tool kind, so that I do not burn a turn on `quickstart(kind="event")`.

## Requirements

**P0**
1. `host.trigger.test(id, body?, headers?)` on a `kind="event"` trigger: if `headers` lacks the trigger's configured verify header (case-insensitive), mcphost serializes `body` once (compact, key order as received — enable `serde_json` `preserve_order` or serialize from the raw JSON-RPC argument text), computes the configured scheme's signature over those bytes with the resolved secret, and proceeds through the existing `verify_event` → `build_run_args` → `insert_queued_run` path unchanged. If the header IS present (even empty), behaviour is exactly today's strict verification.
2. The response becomes `{run_id, status: "queued", test: true, signed: {body: "<exact string signed>", headers: {"<verify header>": "<value>", "content-type": "application/json", "<dedupe_header>": "<test-…>"?}}}` when mcphost signed; `signed` is absent on the strict path. `body` is returned as the exact UTF-8 string, ≤ `event_body_bytes_max` (plan), never re-pretty-printed.
3. `host.trigger.set(kind="event", verify="github" | "stripe")` accepts a string preset expanding to: `github` → `{scheme:"hmac-sha256", header:"X-Hub-Signature-256", prefix:"sha256=", secret:<required `secret` arg, a host.secret_set name>}` with `dedupe_header` defaulting to `X-GitHub-Delivery`; `stripe` → `{scheme:"hmac-sha256", header:"Stripe-Signature", timestamp_header:"t", tolerance_s:300, secret:…}` matching `verify_stripe_style` (`src/hooks.rs:269-306`). `host.trigger.get/list` show the expanded config plus `preset: "github"`. Unknown preset → `trigger_invalid` naming `verify`.
4. `host.runs.list(..., include_result=true)`: for each row whose status is terminal (`done`/`failed`) and whose stored result fits in one part (`parts == 1`, ≤ `RUN_PART_BYTES`), `result` carries the inlined value (same JSON `host.runs.get` returns); otherwise `result: null` and `result_ref` as today. Default `include_result=false` keeps today's shape byte-for-byte. Cap: `limit` is clamped to 50 when `include_result=true` (200 × 256 KiB would be 51 MB).
5. `host.quickstart`'s response and the llms.txt first-run walkthrough gain one sentence: "Triggers (schedule, event, message, webhook) are set with `host.trigger.set(tool, kind=…)`, not a tool kind — see `host.trigger.set`." `host.quickstart(kind="event"|"schedule"|"webhook")` returns `kind_not_registered` with `hint: "did you mean host.trigger.set(kind=\"event\")?"` instead of the bare registered-kinds list.

**P1**
6. `host.trigger.test` for `kind="event"` accepts `body` as a JSON string as well as an object; a string is signed and delivered byte-for-byte (covers senders whose canonical form is not compact JSON).
7. `/healthz` gains `triggers.event_self_tests_total` and `triggers.event_self_test_signature_invalid_total` so the strict-path rejection rate is visible (today only the run row knows).

**P2**
8. `host.trigger.get(id)` on an event trigger returns `sample_curl: "curl -X POST <url> -H 'X-Hub-Signature-256: sha256=…' …"` built from the last self-test's `signed` block, when one exists within 24 h (human copy-paste for the "paste it into GitHub's Redeliver" moment).

## Success metrics

| Metric | Type | Baseline | Target | Method | Timeframe |
|---|---|---|---|---|---|
| `integration-specialist-github-push-webhook-handler` truth-tier score | primary | 0.00 (09-28, 09-29) | ≥ 0.75 | nightly measure.json, after PRD-synthorg-truth-tier-probe-as-tenant lands | first 3 nightly runs after deploy |
| `host.trigger.test` calls per event-trigger session before first `trigger=event` run reaches `done` | primary | 4 (09-28 session), never (09-29) | ≤ 1 | transcript count | same |
| `signature_invalid` from `host.trigger.test` per night across personas | secondary | 2 (one per night) | 0 on the self-sign path | failures.md `last_error_text` | same |
| Bytes returned by `host.runs.list` default call | guardrail | unchanged | unchanged (include_result defaults false) | `http_ac10_concurrency_overhead` style test | at land |
| `POST /hooks/...` accept-path tests | guardrail | all passing | all passing, untouched | `cargo test` events suite | at land |
| `quickstart(kind="event")` occurrences per night | secondary | 1 per night (2 nights) | 0 | failures.md | first 3 runs |
| Real (non-`harness-`) tenants holding ≥ 1 event trigger on prod | guardrail (seed open question) | unknown | counted before any KR | prod `admin.triggers` / healthz counters | before KR |

## Technical considerations

- Live-AC dependency (NOT a build-order gate): this PRD's live recipe AC requires PRD-synthorg-truth-tier-probe-as-tenant's probe fix deployed (landed to synthorg master 2026-09-29) so the truth-tier harness can observe this tool's triggers/runs; the unit/integration ACs are independent and build standalone. Frontmatter Depends-on removed to avoid deadlocking admission on a soft live-only coupling.

- `src/hooks.rs`: `test()` — branch on `headers.contains_key(verify.header)`; factor the signing helpers (`hmac_hex`, `verify_stripe_style` inverse) into `sign_event(scheme, secret, body_bytes, ts?) -> (header_name, header_value)` so `test` and the future `sample_curl` share it. The Stripe preset signs `"{t}.{body}"` exactly as `webhooks.rs:35` does.
- Byte-exact body: either enable `serde_json = { features = ["preserve_order"] }` (repo-wide effect on every `json!` map — audit `tests/` for order-sensitive assertions first) or have `handler.rs` hand `test` the raw argument slice for `body`. Recommend the raw-slice route: the JSON-RPC `params.arguments` are available as `Value` in `handler.rs:3612`; capture the `body` member's original text with `serde_json::value::RawValue` in the tool-call envelope only for `host.trigger.test`.
- `src/handler.rs:1668-1728`: `verify` schema currently `description`-only; add `oneOf` string|object and the preset enum to the description so the tool schema teaches the preset.
- `src/runs.rs:300-315` `list` + `run_to_json`: add `include_result` (bool, default false) → read part 0 via the existing `host.runs.get` inline step (`:224-262`) when `parts == 1`; leave `result_ref` populated in both cases.
- `src/llms_txt.rs`: add the trigger sentence to the first-run walkthrough (the existing `firstpub_ac07_llms_txt_first_run_executes` test replays it — keep the walkthrough executable).
- Positioning (per seed): a fired run inherits mcphost's run model — `trigger: "event"`, `test: true`, `end_user` (`src/runs.rs:152`, `runsubject_ac04_schedule_fired_run_end_user_null.rs`); per-end-user credential resolution via `host.vault.*` stays the mcphost-only capability Backenly lacks. This PRD adds nothing Backenly-shaped (no DB/REST scaffolding).

## Migration / compatibility

- No migration. `signed` and `preset` are additive fields; `include_result` is opt-in. The strict path is unchanged for any caller who supplies the header, so existing Rust tests that sign for themselves keep passing.
- `host.quickstart` error text for unregistered kinds changes only for the three trigger words; `kind_not_registered` code is unchanged.

## Open questions

| # | Question | Owner | Default if unanswered |
|---|---|---|---|
| 1 | `preserve_order` feature vs raw-slice capture for byte-exact bodies? | coder at build | raw-slice for `host.trigger.test` only |
| 2 | Should `signed.body` be omitted above 16 KiB to keep the MCP response small? | Joe | return it up to `event_body_bytes_max`; the caller sent it |
| 3 | Is `X-GitHub-Delivery` dedupe-by-default on the `github` preset desirable for test deliveries (each test mints `test-<ulid>` so no collision)? | Joe | yes |
| 4 | Recipe gold reads `result.payload.commits == 3` while the prompt says "return the commit count" — a tool returning `{"commit_count": 3}` fails the gold. Fix the recipe prompt or the gold (synthorg corpus)? | Joe | align the gold to `result.commit_count == 3` in the synthorg PRD's corpus follow-up |
| 5 | How many real tenants use `kind="event"` today? | runner on orch | unknown; guardrail metric above |

## Acceptance criteria

1. P0 — Given an event trigger with `verify` scheme `hmac-sha256`, header `X-Hub-Signature-256`, prefix `sha256=`, secret name `hook-secret-1`, When `host.trigger.test(id, body={"commits":[{"id":"a"},{"id":"b"},{"id":"c"}]})` is called with no `headers`, Then the response has `status: "queued"`, `test: true`, `signed.headers["X-Hub-Signature-256"] == "sha256=" + hex(HMAC-SHA256(secret, signed.body))`, and `host.runs.wait(run_id)` reaches `done` with the tool having received `event.body.commits` of length 3.
2. P0 — Given the same trigger, When `host.trigger.test` is called with `headers: {"X-Hub-Signature-256": ""}` or a wrong value, Then it fails `signature_invalid` with message `signature in 'X-Hub-Signature-256' did not match` and no run row is created.
3. P0 — Given a self-signed test response, When a client POSTs `signed.body` verbatim with `signed.headers` to the trigger's public `url`, Then `POST /hooks/...` answers 202 with a new run id (the signature verifies on the live path too).
4. P0 — Given `host.trigger.set(tool, kind="event", verify="github", secret="hook-secret-1")`, When `host.trigger.get(id)` is read, Then `verify` shows `{scheme:"hmac-sha256", header:"X-Hub-Signature-256", prefix:"sha256="}`, `preset: "github"`, `dedupe_header: "X-GitHub-Delivery"`, and the secret value is never present.
5. P0 — Given `host.trigger.set(kind="event", verify="paypal")`, When called, Then it fails `trigger_invalid` naming `verify` and listing `github, stripe` as the presets.
6. P0 — Given a `done` run whose result is 1 KiB, When `host.runs.list(trigger="event", include_result=true)` is called, Then that row's `result` equals `host.runs.get(run_id).result`; and When called without `include_result`, Then `result` is `null` and the row is byte-identical to today's shape.
7. P0 — Given a `done` run whose result is 300 KiB (two parts), When `host.runs.list(include_result=true)` is called, Then `result` is `null` and `result_ref.parts == 2`.
8. P0 — Given `host.quickstart(kind="event")`, When called, Then the response carries `hint` containing `host.trigger.set(kind="event")`, and `host.quickstart()`'s text plus llms.txt's first-run walkthrough contain the sentence naming `host.trigger.set` for triggers. (Drafted as a hint carried by an `unknown_kind` *error*; PRD-mcphost-unknown-kind-routes-to-recipe landed on main after this PRD was drafted and turned the same call into a *successful* alias resolution to the http webhook-inbox recipe — kindroute_ac01/ac02, which this PRD must not regress — so the same `hint` field with the same payload now rides on that success response instead of a rejection, and the agent reads the right call without having to recover from an error first.)
9. P1 — Given `body` passed as the string `{"a":1, "b":2}` (with a space), When self-signed, Then `signed.body` is exactly that string and the signature verifies over it.
10. P1 — Given three self-tests and one strict-path rejection, When `/healthz` is read, Then `triggers.event_self_tests_total == 3` and `triggers.event_self_test_signature_invalid_total == 1`.
11. P0 — Given prod mcphost with this PRD deployed and PRD-synthorg-truth-tier-probe-as-tenant landed, When one nightly truth-tier run executes `integration-specialist-github-push-webhook-handler`, Then the persona reaches a `trigger=event` run with `status=done` using ≤ 1 `host.trigger.test` call and the recipe scores ≥ 0.75 (live: nightly `synthorg consume --tier truth` on orch)
