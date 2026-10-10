//! `docs/clients.toml` (PRD-mcphost-docs-external-links-resolve R3): the one
//! list of client install blocks and docs links. `mcphost llms-txt` renders
//! it between `<!-- clients:start -->`/`<!-- clients:end -->` in `README.md`
//! and `www/llms.txt`; `--check` fails when either file's block differs.
//! `install_links` reads each row's `docs_url`/`checked` from here too, so a
//! client's docs link lives in exactly one place.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use serde::Deserialize;

pub const CLIENTS_TOML: &str = include_str!("../docs/clients.toml");
pub const CLIENTS_SECTION_START: &str = "<!-- clients:start -->";
pub const CLIENTS_SECTION_END: &str = "<!-- clients:end -->";

#[derive(Debug, Deserialize)]
pub struct Client {
    pub name: String,
    pub install: String,
    pub docs_url: String,
    pub checked: String,
}

#[derive(Debug, Deserialize)]
pub struct Registry {
    pub search_url: String,
}

#[derive(Debug, Deserialize)]
pub struct ClientsFile {
    pub registry: Registry,
    pub client: Vec<Client>,
}

pub fn parse(src: &str) -> Result<ClientsFile, toml::de::Error> {
    toml::from_str(src)
}

/// The embedded copy of `docs/clients.toml` (compile-time, so `install_links`
/// and the drift tests need no file access).
pub fn builtin() -> &'static ClientsFile {
    static FILE: OnceLock<ClientsFile> = OnceLock::new();
    FILE.get_or_init(|| parse(CLIENTS_TOML).expect("docs/clients.toml must parse"))
}

/// `(docs_url, checked)` for the client called `name`. Panics on an unknown
/// name: `install_links`' table and the toml are one set, and the AC4 drift
/// test pins them together.
pub fn docs_for(name: &str) -> (&'static str, &'static str) {
    let c = builtin()
        .client
        .iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("docs/clients.toml has no [[client]] named {name:?}"));
    (c.docs_url.as_str(), c.checked.as_str())
}

/// The block between the `clients` markers, markers included. The body sits
/// inside `install_links`' own marker pair so `mcphost gen-docs` (which
/// splices that pair) and `mcphost llms-txt` (this) agree byte for byte.
pub fn render_block(file: &ClientsFile) -> String {
    let blocks: Vec<String> = file
        .client
        .iter()
        .map(|c| format!("**{}**\n\n{}\n\nDocs: <{}> (checked {})", c.name, c.install.trim_end(), c.docs_url, c.checked))
        .collect();
    format!(
        "{CLIENTS_SECTION_START}\n{}\n{}\n{}\n{CLIENTS_SECTION_END}",
        crate::install_links::INSTALL_LINKS_SECTION_START,
        blocks.join("\n\n"),
        crate::install_links::INSTALL_LINKS_SECTION_END,
    )
}

/// Replace the section between the `clients` markers in `content` with
/// `block`. `None` when `content` has no (well-ordered) marker pair.
pub fn splice_clients(content: &str, block: &str) -> Option<String> {
    let start = content.find(CLIENTS_SECTION_START)?;
    let end = content.find(CLIENTS_SECTION_END)? + CLIENTS_SECTION_END.len();
    (end > start).then(|| format!("{}{}{}", &content[..start], block.trim_end(), &content[end..]))
}

/// The text between the `clients` markers, markers included.
pub fn block_of(content: &str) -> Option<&str> {
    let start = content.find(CLIENTS_SECTION_START)?;
    let end = content.find(CLIENTS_SECTION_END)? + CLIENTS_SECTION_END.len();
    (end > start).then(|| &content[start..end])
}

/// Every external (non-`mcphost.dev`) http(s) URL in `text`.
pub fn external_urls(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = text;
    while let Some(i) = rest.find("https://").into_iter().chain(rest.find("http://")).min() {
        let tail = &rest[i..];
        let len = tail.find(|c: char| c.is_whitespace() || "<>)]\"'`".contains(c)).unwrap_or(tail.len());
        let url = tail[..len].trim_end_matches(['.', ',', ';', ':']);
        if !url.starts_with("https://mcphost.dev") {
            out.insert(url.to_string());
        }
        rest = &tail[len.max(1)..];
    }
    out
}

/// What `mcphost llms-txt` does for one file's clients block: the file's
/// new content, or an error naming why the block cannot be rendered.
pub fn render_into(content: &str, file: &ClientsFile) -> Result<String, &'static str> {
    splice_clients(content, &render_block(file)).ok_or("missing <!-- clients:start/end --> markers")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_toml_parses_with_thirteen_clients() {
        assert_eq!(builtin().client.len(), 13);
        assert!(builtin().registry.search_url.starts_with("https://"));
    }

    #[test]
    fn splice_is_idempotent_and_leaves_the_rest_alone() {
        let block = render_block(builtin());
        let doc = format!("before\n{block}\nafter\n");
        assert_eq!(render_into(&doc, builtin()).unwrap(), doc);
        assert!(render_into("no markers", builtin()).is_err());
    }
}
