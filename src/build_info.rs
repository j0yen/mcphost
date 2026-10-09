//! PRD-mcphost-healthz-version-field requirement 1: the one place the
//! running build identifies itself. Anonymous `/healthz`, admin `/healthz`
//! and `/status.json` all render [`BUILD`] through [`BuildInfo::json`], so
//! their `version`/`git_sha` values cannot drift apart.

use serde_json::{Map, Value};

pub struct BuildInfo {
    pub version: &'static str,
    /// `None` when built outside a git checkout (`build.rs` leaves
    /// `MCPHOST_GIT_SHA` unset); rendered as JSON `null`, never omitted.
    pub git_sha: Option<&'static str>,
}

pub const BUILD: BuildInfo = BuildInfo {
    version: env!("CARGO_PKG_VERSION"),
    git_sha: option_env!("MCPHOST_GIT_SHA"),
};

impl BuildInfo {
    /// The only renderer: `{"version": "<v>", "git_sha": "<sha>"|null}`.
    pub fn json(&self) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("version".into(), Value::from(self.version));
        m.insert("git_sha".into(), self.git_sha.map_or(Value::Null, Value::from));
        m
    }
}
