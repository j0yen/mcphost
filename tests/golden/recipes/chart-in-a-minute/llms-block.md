<!-- recipe:chart-in-a-minute begin -->
## Recipe calls: Chart in a minute

The exact calls, in order (generated from `recipes/chart-in-a-minute.toml`; do not hand-edit -- run `cargo xtask recipes`). Plan fit: free.

Sign up first (`signup`), once per tenant: `owner`.

1. `host.table.create(columns={"amount":"real","category":"text","day":"integer","id":"integer"}, name="expenses", primary_key="id")`
2. `host.table.append(rows=[{"amount":11.1,"category":"produce","day":1,"id":1},{"amount":7.25,"category":"produce","day":2,"id":2},{"amount":900.0,"category":"rent","day":3,"id":3},{"amount":32.5,"category":"transit","day":4,"id":4}], table="expenses")`
   expect `$.appended` = 4
3. `host.table.chart(share=true, sql="SELECT category, SUM(amount) AS total FROM expenses GROUP BY category")` -- the mark is picked from the columns' types; share_url opens with no login
   expect `$.schema` = "chart.v1"
   expect `$.share_url` is present
   expect `$.vega_lite` is present

Expected cost: 4 calls (the 1 signup included), within 60 s.
<!-- recipe:chart-in-a-minute end -->