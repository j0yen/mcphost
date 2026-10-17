#!/usr/bin/env bash
# GENERATED from recipes/docs-qa.toml by `cargo xtask recipes` -- do not hand-edit.
# Recipe calls (budget: 6 calls, within 90 s):
#   1. host.docs.put
#   2. host.docs.put
#   3. host.docs.status
#   4. host.tool_publish
#   5. host.tool_call
# proof.sh -- runnable end-to-end proof of "Docs Q&A in a minute"
# (see www/llms.txt). A thin wrapper over docs-qa.sh, which uploads the
# 8-document corpus, waits for the index, publishes ask_docs and asks the
# 6 gold questions against $MCPHOST_URL.
#
# Env vars:
#   MCPHOST_URL   the endpoint to run the recipe against.
#                 Default: https://mcphost.dev/mcp
#
# Exit code 0 iff docs-qa.sh exits 0.

set -uo pipefail
cd "$(dirname "$0")"

MCPHOST_URL="${MCPHOST_URL:-https://mcphost.dev/mcp}"
exec bash ./docs-qa.sh "$MCPHOST_URL"
