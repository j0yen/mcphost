## Share a tool, not a key

A team lead's agent wraps one paid API; teammates' agents call it by name.
None of them ever sees the key, and one call revokes any of them.

1. Store the key once, encrypted, never returned by any tool:
   ```
   host.secret_set(name="stripe", value="sk_live_...")
   ```
2. Publish the wrapper. The header references the secret by name; the host
   interpolates it at call time, and callers' `tools/list` entry never
   shows a `secret.*` parameter:
   ```
   host.tool_publish(name="stripe_balance", kind="http", spec={
     "method": "GET", "url": "https://api.stripe.com/v1/balance",
     "headers": {"Authorization": "Bearer {{ secret.stripe }}"}
   })
   ```
3. Create a group for the team:
   ```
   host.group.create(name="finance")
   ```
4. Share the tool to that group:
   ```
   host.tool_share(name="stripe_balance", visibility="group", group="finance")
   ```
5. Add each teammate's tenant by namespace:
   ```
   host.group.add(name="finance", namespace="<teammate_namespace>")
   ```
6. A teammate's agent calls it like any other tool -- no key, ever:
   ```
   host.tool_call(name="<owner_namespace>.stripe_balance", args={})
   ```

Revoke one teammate any time with `host.group.remove(name, namespace)` --
their next call fails, the rest keep working, no republish needed. Runnable
end-to-end proof: `examples/share-a-tool/proof.sh`.

### Share a tool, not a key: public variant

Step 4 can instead be `host.tool_share(name="stripe_balance",
visibility="public")` -- discoverable by anyone via `host.catalog.search`,
no group or step 5 needed. `group` is the default above because a tool
built around `{{ secret.<name> }}` almost always wraps a paid, rate-limited,
or ToS-restricted upstream account: `public` invites call volume from
strangers against that same account, with no allowlist standing between
them and your key's rate limit or bill. Reach for `public` when the tool
has no paid or limited upstream behind it -- a free API, or a tool with no
secrets at all.
