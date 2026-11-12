//! `reach`: what a stored tool can touch (`manifest`, PRD-mcphost-tool-reach-
//! manifest-v2's `host.tool.reach`) and the second public hostname every
//! human-facing link carries as a fallback (`alt_host`, PRD-mcphost-
//! reachability-alt-host). Both halves keep their public paths: `crate::reach::
//! fallback_block`, `crate::reach::reach` and `crate::reach::derive` all resolve here.

mod alt_host;
mod manifest;

pub use alt_host::*;
pub use manifest::*;
