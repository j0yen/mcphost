<!-- recipe:uptime-probes begin -->
## Recipe calls: Uptime probes with no server

The exact calls, in order (generated from `recipes/uptime-probes.toml`; do not hand-edit -- run `cargo xtask recipes`). Plan fit: free.

Sign up first (`signup`), once per tenant: `owner`.

1. `host.uptime.create(name="status", urls=["<fixture_base>/health","<fixture_base>/health?second=1"])` -- creates the probe set, a status tool and one schedule, and probes once
   expect `$.status_tool` = "status"
   expect `$.schedule_id` is present
   expect `$.first_result.rows[0].ok` = true
2. `host.tool_call(args={}, name="status")` -- reads the last stored result, never fetches live

Expected cost: 3 calls (the 1 signup included), within 60 s.
<!-- recipe:uptime-probes end -->