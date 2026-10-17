//! GENERATED from recipes/chart-in-a-minute.toml by `cargo xtask recipes` -- do not hand-edit.
//! PRD-mcphost-recipe-golden-path-tests R4: the `chart-in-a-minute` recipe's exact calls, in
//! order, against an in-process `TestServer` with the recipe's fixture routes
//! mounted; every `expect` must hold and the run must finish under 60 s.

use crate::rgpt;

const RECIPE: &str = r####"{
  "budget": {
    "calls": 4,
    "wall_s": 60
  },
  "fixture_routes": [],
  "https_public": false,
  "name": "chart-in-a-minute",
  "registry": "http",
  "steps": [
    {
      "args": {
        "columns": {
          "amount": "real",
          "category": "text",
          "day": "integer",
          "id": "integer"
        },
        "name": "expenses",
        "primary_key": "id"
      },
      "as": "owner",
      "expect": [],
      "retry_s": null,
      "signed_post": null,
      "tool": "host.table.create"
    },
    {
      "args": {
        "rows": [
          {
            "amount": 11.1,
            "category": "produce",
            "day": 1,
            "id": 1
          },
          {
            "amount": 7.25,
            "category": "produce",
            "day": 2,
            "id": 2
          },
          {
            "amount": 900.0,
            "category": "rent",
            "day": 3,
            "id": 3
          },
          {
            "amount": 32.5,
            "category": "transit",
            "day": 4,
            "id": 4
          }
        ],
        "table": "expenses"
      },
      "as": "owner",
      "expect": [
        {
          "contains": null,
          "exists": null,
          "path": "$.appended",
          "value": 4
        }
      ],
      "retry_s": null,
      "signed_post": null,
      "tool": "host.table.append"
    },
    {
      "args": {
        "share": true,
        "sql": "SELECT category, SUM(amount) AS total FROM expenses GROUP BY category"
      },
      "as": "owner",
      "expect": [
        {
          "contains": null,
          "exists": null,
          "path": "$.schema",
          "value": "chart.v1"
        },
        {
          "contains": null,
          "exists": true,
          "path": "$.share_url",
          "value": null
        },
        {
          "contains": null,
          "exists": true,
          "path": "$.vega_lite",
          "value": null
        }
      ],
      "retry_s": null,
      "signed_post": null,
      "tool": "host.table.chart"
    }
  ],
  "tenants": [
    "owner"
  ]
}"####;

#[tokio::test]
async fn rgpt_chart_in_a_minute_golden_path() {
    let spec: serde_json::Value = serde_json::from_str(RECIPE).unwrap_or_else(|e| panic!("embedded recipe json: {e}"));
    rgpt::run(&spec).await;
}
