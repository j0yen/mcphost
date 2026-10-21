<!-- recipe:database-in-a-minute begin -->
## Recipe calls: Give Claude a database in one minute

The exact calls, in order (generated from `recipes/database-in-a-minute.toml`; do not hand-edit -- run `cargo xtask recipes`). Plan fit: free.

Sign up first (`signup`), once per tenant: `owner`.

1. `host.table.create(columns={"amount":"real","category":"text","day":"integer","id":"integer"}, name="expenses", primary_key="id")`
2. `host.table.append(rows=[{"amount":11.1,"category":"produce","day":1,"id":1},{"amount":7.25,"category":"produce","day":2,"id":2},{"amount":900.0,"category":"rent","day":3,"id":3},{"amount":32.5,"category":"transit","day":4,"id":4}], table="expenses")`
   expect `$.appended` = 4
3. `host.table.model_set(key="description", table="expenses", value="Household spending, one row per purchase.")`
4. `host.table.query(sql="SELECT category, SUM(amount) AS total FROM expenses GROUP BY category ORDER BY category")`
   expect `$.rows[0].category` = "produce"
5. `host.table.query(sql="SELECT COUNT(*) AS n FROM expenses WHERE category LIKE '%prod%'")`
   expect `$.rows[0].n` = 2
6. `host.table.query_log(limit=10)` -- the last calls newest first, each with its sql and row_count
   expect `$.rows[0].row_count` = 1

Expected cost: 7 calls (the 1 signup included), within 60 s.
<!-- recipe:database-in-a-minute end -->