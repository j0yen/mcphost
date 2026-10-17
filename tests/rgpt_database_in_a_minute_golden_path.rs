//! GENERATED from recipes/database-in-a-minute.toml by `cargo xtask recipes` -- do not hand-edit.
//! PRD-mcphost-recipe-golden-path-tests R4: the `database-in-a-minute` recipe's exact calls, in
//! order, against an in-process `TestServer` with the recipe's fixture routes
//! mounted; every `expect` must hold and the run must finish under 60 s.

use crate::rgpt;

const RECIPE: &str = r####"{
  "budget": {
    "calls": 7,
    "wall_s": 60
  },
  "fixture_routes": [],
  "https_public": false,
  "name": "database-in-a-minute",
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
        "key": "description",
        "table": "expenses",
        "value": "Household spending, one row per purchase."
      },
      "as": "owner",
      "expect": [],
      "retry_s": null,
      "signed_post": null,
      "tool": "host.table.model_set"
    },
    {
      "args": {
        "sql": "SELECT category, SUM(amount) AS total FROM expenses GROUP BY category ORDER BY category"
      },
      "as": "owner",
      "expect": [
        {
          "contains": null,
          "exists": null,
          "path": "$.rows[0].category",
          "value": "produce"
        }
      ],
      "retry_s": null,
      "signed_post": null,
      "tool": "host.table.query"
    },
    {
      "args": {
        "sql": "SELECT COUNT(*) AS n FROM expenses WHERE category LIKE '%prod%'"
      },
      "as": "owner",
      "expect": [
        {
          "contains": null,
          "exists": null,
          "path": "$.rows[0].n",
          "value": 2
        }
      ],
      "retry_s": null,
      "signed_post": null,
      "tool": "host.table.query"
    },
    {
      "args": {
        "limit": 10
      },
      "as": "owner",
      "expect": [
        {
          "contains": null,
          "exists": null,
          "path": "$.rows[0].row_count",
          "value": 1
        }
      ],
      "retry_s": null,
      "signed_post": null,
      "tool": "host.table.query_log"
    }
  ],
  "tenants": [
    "owner"
  ]
}"####;

#[tokio::test]
async fn rgpt_database_in_a_minute_golden_path() {
    let spec: serde_json::Value = serde_json::from_str(RECIPE).unwrap_or_else(|e| panic!("embedded recipe json: {e}"));
    rgpt::run(&spec).await;
}
