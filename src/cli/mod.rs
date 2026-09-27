//! CLI subcommand business logic that doesn't fit the "one module per
//! concern" convention every other `src/*.rs` file already uses (`billing`,
//! `funnel`, ...) -- `oauth_probe` is the first subcommand PRD-mcphost-
//! oauth-conformance-harness's own engineering target names a `src/cli/`
//! path for.

pub mod oauth_probe;
