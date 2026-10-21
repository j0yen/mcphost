<!-- recipe:docs-qa begin -->
## Recipe calls: Docs Q&A in a minute

The exact calls, in order (generated from `recipes/docs-qa.toml`; do not hand-edit -- run `cargo xtask recipes`). Plan fit: free.

Sign up first (`signup`), once per tenant: `owner`.

1. `host.docs.put(content="# VPN setup\n\nThe VPN client connects on port 1194 over UDP.\n", name="vpn-setup.md")`
2. `host.docs.put(content="# Onboarding\n\nOn day one you receive a laptop and a badge.\n", name="onboarding.md")`
3. `host.docs.status()` -- poll until index.lag_seconds == 0
   expect `$.index.lag_seconds` = 0
4. `host.tool_publish(kind="python", name="ask_docs", spec={"source":"<contents of examples/docs-qa/ask_docs.py>"})`
5. `host.tool_call(args={"k":5,"query":"What port does the VPN client connect on?"}, name="ask_docs")` -- every passage carries a name:offset citation
   expect `$.passages[0].name` = "vpn-setup.md"

Expected cost: 6 calls (the 1 signup included), within 90 s.
<!-- recipe:docs-qa end -->