//! `mcphost`: one MCP endpoint an agent can join and administer without a
//! human. See `README.md` for the product framing; this crate is the
//! streamable-HTTP server, its SQLite-backed tenancy/control-plane, and the
//! `Kind` trait + registry that feature crates (REST-wrapper tools, code
//! tools) extend via `build_into`.

pub mod admin;
pub mod auth;
pub mod control;
pub mod db;
pub mod errors;
pub mod handler;
pub mod http;
pub mod kinds;
pub mod registry;
pub mod sandbox;
pub mod secrets;
pub mod state;
