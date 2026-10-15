# mcphost-task-handoff-live-script — live receipt

AC 3 (Live): `bash scripts/task-handoff-live.sh https://mcphost.dev`, run from
the landed worktree on the build host (only bash, curl, python3), plus the
`/healthz` version of mcphost-1 after deploy.

Status: **pending the build host's live step.** The live step replaces the
sections below with the real `/healthz` version and the stdout of the command
above. Nothing in this file is a prod result until it does.

## mcphost-1 healthz version after deploy

(pending live step)

## stdout of `bash scripts/task-handoff-live.sh https://mcphost.dev`

(pending live step)

## Offline reference (not the AC 3 evidence)

Same script against a locally spawned `mcphost serve` (debug build, ephemeral
port), run during the build:

```
status: done
latency_ms: 255
```

The offline proof that keeps the script from drifting from `host.task.*` is
`tests/thlive_ac02_local_server_handoff_done.rs`.
