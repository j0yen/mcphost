<!-- recipe:share-a-tool begin -->
## Recipe calls: Share a tool, not a key

The exact calls, in order (generated from `recipes/share-a-tool.toml`; do not hand-edit -- run `cargo xtask recipes`). Plan fit: free.

Sign up first (`signup`), once per tenant: `owner`, `teammate`.

1. `host.secret_set(name="upstream_key", value="sk_live_fixture_not_a_real_key")` [owner]
2. `host.tool_publish(kind="http", name="paid_api", spec={"headers":{"Authorization":"Bearer {{ secret.upstream_key }}"},"method":"GET","response":"json","url":"<fixture_base>/v1/balance"})` [owner]
3. `host.group.create(name="finance")` [owner]
4. `host.tool_share(group="finance", name="paid_api", visibility="group")` [owner]
5. `host.group.add(name="finance", namespace="<teammate_namespace>")` [owner]
6. `host.tool_call(args={}, name="<owner_namespace>.paid_api")` [teammate] -- the teammate calls it by name -- no key, ever
   expect `$.payload.object` = "balance"

Expected cost: 8 calls (the 2 signups included), within 60 s.
<!-- recipe:share-a-tool end -->