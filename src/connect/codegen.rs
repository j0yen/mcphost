//! The `providers.toml` parser and Rust generator, shared by `build.rs`
//! (which `#[path]`-includes this file, so it must stay free of crate
//! imports) and the library (tests re-run it to prove the compiled table is
//! exactly what the generator emits for the committed TOML).

use std::collections::BTreeMap;
use std::fmt::Write as _;

/// One validated `providers.toml` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub name: String,
    pub auth_url: String,
    pub token_url: String,
    /// `(name, purpose)`, in file order.
    pub scopes: Vec<(String, String)>,
    pub refresh: String,
    pub client_id_env: String,
    pub client_secret_env: String,
    pub extra_authorize_params: BTreeMap<String, String>,
    pub docs_url: String,
}

const REQUIRED_STRING_FIELDS: [&str; 6] = [
    "auth_url",
    "token_url",
    "refresh",
    "client_id_env",
    "client_secret_env",
    "docs_url",
];

pub const REFRESH_KINDS: [&str; 3] = ["rotating", "offline_access", "none"];

/// Parse and validate `providers.toml` text. Every failure names the
/// provider and the field (AC7), so `build.rs` can fail the build with it.
pub fn parse_table(text: &str) -> Result<Vec<Row>, String> {
    let root: toml::Table = text.parse().map_err(|e| format!("providers.toml: not valid TOML: {e}"))?;
    let mut rows = Vec::new();
    for (name, value) in &root {
        let valid_name = name.starts_with(|c: char| c.is_ascii_lowercase())
            && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        if !valid_name {
            return Err(format!(
                "providers.toml: provider '{name}': name must match ^[a-z][a-z0-9_]*$"
            ));
        }
        let table = value
            .as_table()
            .ok_or_else(|| format!("providers.toml: provider '{name}': expected a table"))?;
        let mut strings = Vec::new();
        for field in REQUIRED_STRING_FIELDS {
            let v = table
                .get(field)
                .ok_or_else(|| format!("providers.toml: provider '{name}' is missing field '{field}'"))?;
            let s = v
                .as_str()
                .ok_or_else(|| format!("providers.toml: provider '{name}': field '{field}' must be a string"))?;
            if s.is_empty() {
                return Err(format!("providers.toml: provider '{name}': field '{field}' is empty"));
            }
            strings.push(s.to_string());
        }
        let refresh = strings[2].clone();
        if !REFRESH_KINDS.contains(&refresh.as_str()) {
            return Err(format!(
                "providers.toml: provider '{name}': field 'refresh' must be one of {}, got '{refresh}'",
                REFRESH_KINDS.join(" | ")
            ));
        }
        let scope_items = table
            .get("scopes")
            .ok_or_else(|| format!("providers.toml: provider '{name}' is missing field 'scopes'"))?
            .as_array()
            .ok_or_else(|| format!("providers.toml: provider '{name}': field 'scopes' must be an array"))?;
        let mut scopes = Vec::new();
        for item in scope_items {
            let field = |key: &str| {
                item.get(key).and_then(toml::Value::as_str).map(str::to_string).ok_or_else(|| {
                    format!("providers.toml: provider '{name}': every scopes entry needs a string '{key}'")
                })
            };
            scopes.push((field("name")?, field("purpose")?));
        }
        let mut extra = BTreeMap::new();
        if let Some(params) = table.get("extra_authorize_params") {
            let params = params.as_table().ok_or_else(|| {
                format!("providers.toml: provider '{name}': field 'extra_authorize_params' must be a table")
            })?;
            for (k, v) in params {
                let v = v.as_str().ok_or_else(|| {
                    format!("providers.toml: provider '{name}': extra_authorize_params.{k} must be a string")
                })?;
                extra.insert(k.clone(), v.to_string());
            }
        }
        rows.push(Row {
            name: name.clone(),
            auth_url: strings[0].clone(),
            token_url: strings[1].clone(),
            scopes,
            refresh,
            client_id_env: strings[3].clone(),
            client_secret_env: strings[4].clone(),
            extra_authorize_params: extra,
            docs_url: strings[5].clone(),
        });
    }
    if rows.is_empty() {
        return Err("providers.toml: no providers defined".to_string());
    }
    Ok(rows)
}

fn variant_of(name: &str) -> String {
    name.split('_')
        .map(|part| {
            let mut c = part.chars();
            c.next().map(|f| f.to_ascii_uppercase().to_string() + c.as_str()).unwrap_or_default()
        })
        .collect()
}

fn refresh_variant(refresh: &str) -> &'static str {
    match refresh {
        "rotating" => "Refresh::Rotating",
        "offline_access" => "Refresh::OfflineAccess",
        _ => "Refresh::None",
    }
}

/// The generated Rust: `enum Provider`, `Provider::ALL`, `Provider::parse`,
/// one associated const per provider, and the static `ProviderSpec` table.
/// Deterministic, so a test can regenerate and diff.
pub fn render_rust(rows: &[Row]) -> String {
    let mut o = String::new();
    o.push_str("// @generated by build.rs from providers.toml -- do not edit.\n\n");
    o.push_str("#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]\npub enum Provider {\n");
    for r in rows {
        let _ = writeln!(o, "    {},", variant_of(&r.name));
    }
    o.push_str("}\n\nimpl Provider {\n    pub const ALL: &'static [Provider] = &[\n");
    for r in rows {
        let _ = writeln!(o, "        Provider::{},", variant_of(&r.name));
    }
    o.push_str("    ];\n\n");
    for r in rows {
        let _ = writeln!(
            o,
            "    pub const {}: Provider = Provider::{};",
            r.name.to_ascii_uppercase(),
            variant_of(&r.name)
        );
    }
    o.push_str("\n    pub fn parse(s: &str) -> Option<Provider> {\n        match s {\n");
    for r in rows {
        let _ = writeln!(o, "            {:?} => Some(Provider::{}),", r.name, variant_of(&r.name));
    }
    o.push_str("            _ => None,\n        }\n    }\n\n    pub fn spec(self) -> &'static ProviderSpec {\n        match self {\n");
    for (i, r) in rows.iter().enumerate() {
        let _ = writeln!(o, "            Provider::{} => &SPECS[{i}],", variant_of(&r.name));
    }
    o.push_str("        }\n    }\n}\n\n");
    let _ = writeln!(o, "pub static SPECS: [ProviderSpec; {}] = [", rows.len());
    for r in rows {
        o.push_str("    ProviderSpec {\n");
        let _ = writeln!(o, "        name: {:?},", r.name);
        let _ = writeln!(o, "        auth_url: {:?},", r.auth_url);
        let _ = writeln!(o, "        token_url: {:?},", r.token_url);
        o.push_str("        scopes: &[\n");
        for (n, p) in &r.scopes {
            let _ = writeln!(o, "            ScopeSpec {{ name: {n:?}, purpose: {p:?} }},");
        }
        o.push_str("        ],\n");
        let _ = writeln!(o, "        refresh: {},", refresh_variant(&r.refresh));
        let _ = writeln!(o, "        client_id_env: {:?},", r.client_id_env);
        let _ = writeln!(o, "        client_secret_env: {:?},", r.client_secret_env);
        o.push_str("        extra_authorize_params: &[\n");
        for (k, v) in &r.extra_authorize_params {
            let _ = writeln!(o, "            ({k:?}, {v:?}),");
        }
        o.push_str("        ],\n");
        let _ = writeln!(o, "        docs_url: {:?},", r.docs_url);
        o.push_str("    },\n");
    }
    o.push_str("];\n");
    o
}
