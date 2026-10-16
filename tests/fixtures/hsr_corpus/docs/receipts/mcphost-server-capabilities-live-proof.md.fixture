# mcphost-server-capabilities-live-proof -- live receipt

Status: pending the post-deploy run (Live AC 2, operator-provisioned).

Command, run from the landed worktree on the build host:

    curl -s https://mcphost.dev/healthz
    curl -s -X POST https://mcphost.dev/mcp -H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream' -d @tests/fixtures/clive_complete_keyless.json | jq -e '.error.code == -32601 and (.error.data.help_url | test("capability_unsupported"))'

Paste the healthz version and the command's stdout below after deploy.
