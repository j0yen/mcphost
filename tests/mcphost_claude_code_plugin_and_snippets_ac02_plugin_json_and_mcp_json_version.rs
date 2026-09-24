//! AC2 (PRD-mcphost-claude-code-plugin-and-snippets) — Given
//! `plugin/.claude-plugin/plugin.json` and `plugin/.mcp.json`, When parsed
//! with `jq`, Then both succeed, `.name == "mcphost"`, and `.version`
//! equals the `version` in Cargo.toml.
//!
//! Uses `include_str!` (not filesystem reads) so the assertions run
//! against whatever the test binary was actually compiled with, on
//! whichever box runs `cargo test`.

const PLUGIN_JSON: &str = include_str!("../plugin/.claude-plugin/plugin.json");
const MCP_JSON: &str = include_str!("../plugin/.mcp.json");
const CARGO_TOML: &str = include_str!("../Cargo.toml");

fn cargo_toml_version() -> String {
    let doc: toml::Value = toml::from_str(CARGO_TOML).expect("Cargo.toml must parse as TOML");
    doc["package"]["version"]
        .as_str()
        .expect("Cargo.toml [package].version must be a string")
        .to_string()
}

#[test]
fn plugin_json_parses_and_has_correct_name() {
    let v: serde_json::Value =
        serde_json::from_str(PLUGIN_JSON).expect("plugin/.claude-plugin/plugin.json must be valid JSON");
    assert_eq!(v["name"], "mcphost", "plugin.json .name must be \"mcphost\"");
}

#[test]
fn mcp_json_parses() {
    let v: serde_json::Value =
        serde_json::from_str(MCP_JSON).expect("plugin/.mcp.json must be valid JSON");
    assert!(
        v["mcpServers"]["mcphost"]["url"].is_string(),
        "plugin/.mcp.json must declare an mcphost server with a url"
    );
}

#[test]
fn plugin_json_version_matches_cargo_toml() {
    let v: serde_json::Value =
        serde_json::from_str(PLUGIN_JSON).expect("plugin/.claude-plugin/plugin.json must be valid JSON");
    let plugin_version = v["version"]
        .as_str()
        .expect("plugin.json .version must be a string");
    assert_eq!(
        plugin_version,
        cargo_toml_version(),
        "plugin.json version must equal Cargo.toml's [package].version"
    );
}
