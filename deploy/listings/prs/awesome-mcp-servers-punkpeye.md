# PR draft: punkpeye/awesome-mcp-servers

Target repo: https://github.com/punkpeye/awesome-mcp-servers
Target file: README.md
Section: `### 💻 <a name="developer-tools"></a>Developer Tools` (alphabetical
by the bracketed `org/repo` link text; insert in order among existing
entries in that section)

One-line entry to insert (matches the section's existing bullet format —
link, optional badges, one-sentence description):

```
- [j0yen/mcphost](https://github.com/j0yen/mcphost) ☁️ - Hosted MCP runtime where the agent is the operator: sign up by tool call, then publish, call, and manage your own tools in your own namespace at runtime — no local install, no restart to add a tool.
```

Process (per the list's CONTRIBUTING conventions and PRD technical
considerations — "prepare them in a fork under j0yen and open PRs only
under PUBLISH-OK"):

1. Fork `punkpeye/awesome-mcp-servers` to `j0yen/awesome-mcp-servers` (if
   not already forked).
2. Branch `add-mcphost`, insert the line above in alphabetical position
   in the Developer Tools section.
3. Open a PR titled "Add mcphost" against `punkpeye/awesome-mcp-servers`.
4. Record the PR URL back into `deploy/listings/manifest.yaml`'s
   `awesome-mcp-servers-punkpeye` entry's `submitted` date via
   `scripts/listings-submit.sh`.

Not yet executed — this file is the prepared artifact only. Nothing is
submitted without `PUBLISH-OK` (see `scripts/listings-submit.sh`).
