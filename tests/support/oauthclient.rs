//! Thin re-export of the reusable OAuth client simulator
//! (`src/oauthclient.rs`, compiled into the release binary too, behind
//! `mcphost oauth-probe`) under the path PRD-mcphost-oauth-conformance-
//! harness's own requirement 1 names -- `tests/oauthconf_*.rs` files
//! `use crate::oauthclient::...` after `#[path = "support/oauthclient.rs"]
//! mod oauthclient;`, same convention as `tests/support/oauth.rs`.
#![allow(dead_code, unused_imports)]

pub use mcphost::oauthclient::*;
