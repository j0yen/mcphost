# PR draft: wong2/awesome-mcp-servers

Target repo: https://github.com/wong2/awesome-mcp-servers
Target file: README.md
Section: whichever category README.md's own TOC nearest matches "hosting
runtimes" / general server tooling at PR time — wong2's list re-derives
its category boundaries more often than punkpeye's, so the exact section
name is confirmed at PR-open time, not hardcoded here.

One-line entry to insert (adapt to the target list's exact bullet format
at PR-open time):

```
- [mcphost](https://github.com/j0yen/mcphost) - Hosted MCP runtime where the agent is the operator: sign up by tool call, then publish, call, and manage your own tools in your own namespace at runtime.
```

Process: fork under `j0yen`, branch `add-mcphost`, insert alphabetically,
open PR titled "Add mcphost". Record the PR URL into
`deploy/listings/manifest.yaml`'s `awesome-mcp-wong2` entry's `submitted`
date via `scripts/listings-submit.sh`.

Not yet executed — this file is the prepared artifact only. Nothing is
submitted without `PUBLISH-OK` (see `scripts/listings-submit.sh`).
