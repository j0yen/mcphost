<!-- recipe:first-run begin -->
## Recipe calls: First run: plugin walk-through

The exact calls, in order (generated from `recipes/first-run.toml`; do not hand-edit -- run `cargo xtask recipes`). Plan fit: free.

Sign up first (`signup`), once per tenant: `owner`.

1. `host.tool_publish(kind="python", name="table_note", spec={"source":"<contents of examples/first-run/tools/table_note.py>"})` -- a real python tool, not a stub
   expect `$.kind` = "python"
2. `host.tool_test(args={"note":"hello"}, name="table_note")` -- a real write through the sandbox API
   expect `$.result.appended` = 1
   expect `$.result.table` = "quickstart_notes"
3. `host.trigger.set(kind="schedule", schedule="0 * * * *", tool="table_note")`

Expected cost: 4 calls (the 1 signup included), within 90 s.
<!-- recipe:first-run end -->