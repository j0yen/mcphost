//! `mcphost`: one MCP endpoint an agent can join and administer without a
//! human. See `README.md` for the product framing; this crate is the
//! streamable-HTTP server, its SQLite-backed tenancy/control-plane, and the
//! `Kind` trait + registry that feature crates (REST-wrapper tools, code
//! tools) extend via `build_into`.

pub mod admin;
pub mod agents;
pub mod api_contract;
pub mod auth;
pub mod bans;
pub mod billing;
pub mod channels;
pub mod claim;
pub mod compat_check;
pub mod consent;
pub mod control;
pub mod cron;
pub mod db;
pub mod deps;
pub mod difftext;
pub mod email;
pub mod errors;
pub mod export;
pub mod funnel;
pub mod handler;
pub mod hooks;
pub mod http;
pub mod kinds;
pub mod llms_txt;
pub mod messaging;
pub mod metering;
pub mod network_policy;
pub mod plans;
pub mod registry;
pub mod retention;
pub mod runs;
pub mod sandbox;
pub mod secrets;
pub mod sharing;
pub mod state;
pub mod tables;
pub mod tenant_state;
pub mod triggers;
pub mod webhooks;
