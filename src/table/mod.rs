//! Table-store features that sit beside `tables.rs`: the CSV / JSON Lines import `host.table.exec` (one INSERT, UPDATE or DELETE, plus the statement grammar every table-SQL refusal carries) and `host.table.find` (query by example).
pub mod import;
pub mod exec;
pub mod find;
