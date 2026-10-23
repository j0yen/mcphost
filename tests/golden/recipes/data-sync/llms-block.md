<!-- recipe:data-sync begin -->
## Recipe calls: Data sync: mirror a CSV into a table

The exact calls, in order (generated from `recipes/data-sync.toml`; do not hand-edit -- run `cargo xtask recipes`). Plan fit: free.

Sign up first (`signup`), once per tenant: `owner`.

1. `host.sync.create(source=<a csv-over-http source, ref = the public url of your CSV>, table="orders", mode="pull")` -- registers the source and creates the `orders` table; the host does not fetch at create time
   expect `$.sync_id` is present
   expect `$.next_run_unix` is present
   expect `$.status` is present
   expect `$.table` = "orders"
   expect `$.existing` = false
2. `host.sync.run_now(sync_id="<step1.sync_id>")` -- makes the sync due now; the trigger tick picks it up
   expect `$.next_run_unix` is present
3. `host.sync.get(sync_id="<step1.sync_id>")` -- the sync row; polled until the first receipt (`last_run`) exists: `ok`, 12 rows added, change detected
   expect `$.table` = "orders"
   expect `$.last_run.run_id` is present
   expect `$.last_run.started_unix` is present
   expect `$.last_run.status` = "ok"
   expect `$.last_run.change_detected` = true
   expect `$.last_run.rows_added` = 12
4. `host.sync.run_now(sync_id="<step1.sync_id>")` -- makes the sync due again; the CSV has not changed
   expect `$.next_run_unix` is present
5. `host.sync.get(sync_id="<step1.sync_id>")` -- polled until the second receipt exists: `ok`, no change detected, 0 rows added
   expect `$.table` = "orders"
   expect `$.last_run.status` = "ok"
   expect `$.last_run.change_detected` = false
   expect `$.last_run.rows_added` = 0
6. `host.sync.list()`
   expect `$.syncs[0].table` = "orders"
7. `host.sync.delete(sync_id="<step1.sync_id>")` -- stops the schedule; the `orders` table stays
   expect `$.deleted` = true
   expect `$.table_kept` = true

Expected cost: 8 calls (the 1 signup included), within 95 s.
<!-- recipe:data-sync end -->