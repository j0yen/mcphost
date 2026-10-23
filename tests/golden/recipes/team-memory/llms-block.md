<!-- recipe:team-memory begin -->
## Recipe calls: Give your agents one memory

The exact calls, in order (generated from `recipes/team-memory.toml`; do not hand-edit -- run `cargo xtask recipes`). Plan fit: free.

Sign up first (`signup`), once per tenant: `owner`, `teammate`.

1. `host.table.create(columns={"at":"real","key":"text","tags":"json","text":"text","writer":"text"}, name="memory")` [owner]
2. `host.tool_publish(kind="python", name="remember", spec={"source":"<contents of examples/team-memory/tools/remember.py>"})` [owner]
3. `host.tool_publish(kind="python", name="recall", spec={"source":"<contents of examples/team-memory/tools/recall.py>"})` [owner]
4. `host.group.create(name="team")` [owner]
5. `host.tool_share(group="team", name="remember", visibility="group")` [owner]
6. `host.tool_share(group="team", name="recall", visibility="group")` [owner]
7. `host.group.add(name="team", namespace="<teammate_namespace>")` [owner]
8. `host.tool_call(args={"key":"deploy","text":"deploy window is Tuesday 9pm","who":"<teammate_namespace>"}, name="<owner_namespace>.remember")` [teammate] -- writer holds the caller's own namespace
9. `host.tool_call(args={"query":"deploy"}, name="<owner_namespace>.recall")` [teammate]
   expect `$.rows[0].key` = "deploy"

Expected cost: 11 calls (the 2 signups included), within 90 s.
<!-- recipe:team-memory end -->