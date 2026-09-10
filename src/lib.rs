//! `mcphost`: one MCP endpoint an agent can join and administer without a
//! human. See `README.md` for the product framing; this crate is the
//! streamable-HTTP server, its SQLite-backed tenancy/control-plane, and the
//! `Kind` trait + registry that feature crates (REST-wrapper tools, code
//! tools) extend via `build_into`.

pub mod admin;
pub mod auth;
pub mod billing;
pub mod compat_check;
pub mod control;
pub mod db;
pub mod errors;
pub mod funnel;
pub mod handler;
pub mod http;
pub mod kinds;
pub mod llms_txt;
pub mod metering;
pub mod plans;
pub mod registry;
pub mod runs;
pub mod sandbox;
pub mod secrets;
pub mod sharing;
pub mod state;
pub mod tenant_state;
