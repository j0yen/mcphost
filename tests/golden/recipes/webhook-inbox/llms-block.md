<!-- recipe:webhook-inbox begin -->
## Recipe calls: Webhook inbox in one call

The exact calls, in order (generated from `recipes/webhook-inbox.toml`; do not hand-edit -- run `cargo xtask recipes`). Plan fit: free.

Sign up first (`signup`), once per tenant: `owner`.

1. `host.webhook_inbox.create(name="inbox")` -- returns url, secret and wait in one call
   expect `$.url` contains "/hook/"
   expect `$.secret` is present
   expect `$.wait.tool` = "host.msg.wait"
2. POST `{"id":"evt-fixture-1"}` to `<step1.url>`, signed (HMAC-SHA256 under the secret, `X-Mcphost-Signature: sha256=<hex>`), expect HTTP 200 -- a signed POST, exactly as a sender would
3. POST `{"id":"evt-fixture-1"}` to `<step1.url>`, unsigned, expect HTTP 401 -- an unsigned POST is refused and stores nothing
4. `host.tool_call(args={}, name="inbox")` -- the last 50 stored events as rows
   expect `$.rows[0].id` = "evt-fixture-1"
5. `host.msg.wait()` -- pass wait.args from step 1 as-is
   expect `$.messages[0].data.event_id` = "evt-fixture-1"

Expected cost: 4 calls (the 1 signup included), within 90 s.
<!-- recipe:webhook-inbox end -->