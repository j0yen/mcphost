//! PRD-mcphost-delegation-recipes-and-quickstart R1: the "sub-agent key"
//! recipe, generated from the scope registry, the `host.key.delegate`
//! schema and the delegation error-code constants -- never typed.
//!
//! A [`Recipe`] is three calls an agent can send verbatim. `host.quickstart`
//! returns it, `mcphost llms-txt` renders it into `www/llms.txt` and
//! `README.md`, and `tests/dlrec_ac01_*` executes it against a test server,
//! so the prose cannot name a scope, field or code the server does not use.

use serde::Serialize;
use serde_json::{Value, json};

use crate::scopes::{self, KEY_REVOKED, SCOPE_ESCALATION, Scope, ScopeRow};

pub const SUB_AGENT_KEY_ID: &str = "sub_agent_key";
pub const SUB_AGENT_KEY_TITLE: &str = "Give a sub-agent one key, not yours";

/// The label the recipe gives the helper's key.
const HELPER_LABEL: &str = "helper";
/// The lifetime the recipe asks for: inside every plan's `delegated_ttl_max_s`.
const HELPER_TTL_S: i64 = 3600;
/// The lifetime of the child key the helper asks for (and is refused).
const ESCALATION_TTL_S: i64 = 60;

/// Who sends a call: the lead (the tenant key) or the helper (the key call 1 returned).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Caller {
    Lead,
    Helper,
}

/// `field >= min` on a successful response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AtLeast {
    pub field: String,
    pub min: i64,
}

/// What a call must return: fields on success, or an exact error code.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Expect {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ok_fields: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ok_at_least: Option<AtLeast>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    /// After this call the helper's own next call fails with this code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub helper_next_call_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RecipeCall {
    pub tool: String,
    #[serde(rename = "as")]
    pub caller: Caller,
    pub arguments: Value,
    pub expect: Expect,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Recipe {
    pub id: String,
    pub title: String,
    pub calls: Vec<RecipeCall>,
    /// Plain-text lines; the last is [`HELPER_HINT`] verbatim.
    pub prose: Vec<String>,
}

/// The line `host.whoami` returns (as `hint`, with `<label>` filled in) to a
/// delegated key used from a session other than the minting one (R6). It is
/// the last [`Recipe::prose`] line, so the recipe and the hint share one source.
pub const HELPER_HINT: &str =
    "this key was delegated for <label>; ask your lead for a run-bound key if you need more";

/// [`HELPER_HINT`] for the key labelled `label`.
pub fn delegated_hint(label: &str) -> String {
    HELPER_HINT.replace("<label>", label)
}

/// The recipe for the live scope registry.
pub fn sub_agent_key() -> Recipe {
    sub_agent_key_for(&scopes::catalog())
}

/// The recipe for `catalog`: the helper holds `tools:call`; the escalation it
/// attempts is the first non-root catalog scope `tools:call` does not grant,
/// and the prose names every such scope.
pub fn sub_agent_key_for(catalog: &[ScopeRow]) -> Recipe {
    let held = Scope::ToolsCall.as_str();
    let lacking: Vec<&str> = catalog
        .iter()
        .filter(|r| !r.granted_by(held) && r.name != Scope::Mcp.as_str())
        .map(|r| r.name.as_str())
        .collect();
    // The escalation attempt asks for every scope `tools:call` lacks at once,
    // so each scope the prose names is one the executed call actually asks for.
    let wider: Vec<&str> = if lacking.is_empty() { vec![Scope::ToolsWrite.as_str()] } else { lacking.clone() };

    let calls = vec![
        RecipeCall {
            tool: "host.key.delegate".to_string(),
            caller: Caller::Lead,
            arguments: json!({
                "scopes": [held],
                "ttl_s": HELPER_TTL_S,
                "label": HELPER_LABEL,
                "can_delegate": true,
            }),
            expect: Expect { ok_fields: vec!["key".into(), "label".into()], ..Expect::default() },
        },
        RecipeCall {
            tool: "host.key.delegate".to_string(),
            caller: Caller::Helper,
            arguments: json!({"scopes": wider, "ttl_s": ESCALATION_TTL_S}),
            expect: Expect { error_code: Some(SCOPE_ESCALATION.to_string()), ..Expect::default() },
        },
        RecipeCall {
            tool: "host.key.revoke".to_string(),
            caller: Caller::Lead,
            arguments: json!({"label": HELPER_LABEL, "cascade": true}),
            expect: Expect {
                ok_at_least: Some(AtLeast { field: "revoked".into(), min: 1 }),
                helper_next_call_error: Some(KEY_REVOKED.to_string()),
                ..Expect::default()
            },
        },
    ];
    let prose = vec![
        format!(
            "Hand the helper the `key` call 1 returned, never your own. It holds `{held}` only: any other \
             scope fails `{SCOPE_ESCALATION}`, as call 2 shows."
        ),
        format!("Call 3 revokes it and its children; the helper's next call fails `{KEY_REVOKED}`."),
        HELPER_HINT.to_string(),
    ];
    Recipe {
        id: SUB_AGENT_KEY_ID.to_string(),
        title: SUB_AGENT_KEY_TITLE.to_string(),
        calls,
        prose,
    }
}

impl Expect {
    /// The expectation as one sentence fragment, e.g. "returns `key`, `label`".
    pub fn describe(&self) -> String {
        if let Some(code) = &self.error_code {
            return format!("fails `{code}`");
        }
        let mut parts: Vec<String> = Vec::new();
        if !self.ok_fields.is_empty() {
            parts.push(format!(
                "returns {}",
                self.ok_fields.iter().map(|f| format!("`{f}`")).collect::<Vec<_>>().join(", ")
            ));
        }
        if let Some(a) = &self.ok_at_least {
            parts.push(format!("`{}` >= {}", a.field, a.min));
        }
        parts.join("; ")
    }
}

/// Weights that order `host.quickstart`'s `next_steps`: lowest first, so
/// delegating a key is always offered before sharing a tool (which would hand
/// a teammate access to the tenant's own secrets).
pub const NEXT_STEP_DELEGATE_WEIGHT: u32 = 10;
pub const NEXT_STEP_SHARE_WEIGHT: u32 = 20;

/// `host.quickstart`'s `next_steps` for a plan that allows delegation, sorted
/// by weight.
pub fn next_steps() -> Vec<Value> {
    let mut steps = vec![
        (NEXT_STEP_SHARE_WEIGHT, "share a tool", "host.tool.share"),
        (NEXT_STEP_DELEGATE_WEIGHT, "delegate a key", "host.key.delegate"),
    ];
    steps.sort_by_key(|(w, _, _)| *w);
    steps.into_iter().map(|(weight, step, call)| json!({"step": step, "call": call, "weight": weight})).collect()
}

/// One `curl` per call, for shells (R7), after an array holding the URL and headers. `$MCPHOST_KEY`
/// is the lead's key and `$HELPER_KEY` the `key` call 1 returned.
pub fn copy_paste(recipe: &Recipe, public_url: &str) -> String {
    let url = format!("{}/mcp", public_url.trim_end_matches('/'));
    let mut lines = vec![
        format!("H=({url} -H 'Accept: application/json, text/event-stream' -H 'Content-Type: application/json')"),
    ];
    lines.extend(recipe.calls.iter().map(|c| {
        let key = match c.caller {
            Caller::Lead => "$MCPHOST_KEY",
            Caller::Helper => "$HELPER_KEY",
        };
        let body = json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": c.tool, "arguments": c.arguments},
        });
        format!("curl -s \"${{H[@]}}\" -H \"Authorization: Bearer {key}\" -d '{body}'")
    }));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kinds::KindRegistry;

    /// R1: nothing in the recipe is typed -- every tool, argument name and
    /// scope is reachable from the registry or the `host.key.*` schemas.
    #[test]
    fn recipe_strings_are_reachable_from_registry_and_schema() {
        let recipe = sub_agent_key();
        let tools = crate::handler::host_tool_descriptors(&KindRegistry::with_builtin());
        for call in &recipe.calls {
            let tool = tools
                .iter()
                .find(|t| t.name.as_ref() == call.tool)
                .unwrap_or_else(|| panic!("{} is not a registered tool", call.tool));
            let props = tool.input_schema.get("properties").and_then(Value::as_object).expect("properties");
            for (arg, value) in call.arguments.as_object().expect("object args") {
                let prop = props.get(arg).unwrap_or_else(|| panic!("{}: no schema property {arg}", call.tool));
                if arg == "scopes" {
                    let allowed: Vec<&str> = prop["items"]["enum"]
                        .as_array()
                        .expect("scopes enum")
                        .iter()
                        .filter_map(Value::as_str)
                        .collect();
                    for s in value.as_array().expect("scopes array") {
                        let s = s.as_str().expect("scope string");
                        assert!(allowed.contains(&s), "{s} not in the schema enum");
                        assert!(Scope::parse(s).is_some(), "{s} not in Scope::ALL");
                    }
                }
            }
        }
        let codes = [SCOPE_ESCALATION, KEY_REVOKED];
        let text = recipe.prose.join("\n");
        for code in codes {
            assert!(text.contains(code), "{code} missing from prose");
        }
        let mut allowed_words: Vec<String> = codes.iter().map(|c| c.to_string()).collect();
        allowed_words.extend(scopes::catalog().into_iter().map(|r| r.name));
        allowed_words.push("key".to_string());
        for word in text.split('`').skip(1).step_by(2) {
            assert!(
                allowed_words.iter().any(|a| a == word),
                "backticked `{word}` in prose is neither a scope nor a delegation code"
            );
        }
    }

    #[test]
    fn delegating_is_offered_before_sharing() {
        const { assert!(NEXT_STEP_DELEGATE_WEIGHT < NEXT_STEP_SHARE_WEIGHT) };
        let steps = next_steps();
        let at = |name: &str| steps.iter().position(|s| s["step"] == name).expect(name);
        assert!(at("delegate a key") < at("share a tool"));
    }

    #[test]
    fn hint_is_the_last_prose_line() {
        let recipe = sub_agent_key();
        assert_eq!(recipe.prose.last().map(String::as_str), Some(HELPER_HINT));
        assert_eq!(delegated_hint("scout"), "this key was delegated for scout; ask your lead for a run-bound key if you need more");
    }

    #[test]
    fn a_new_scope_changes_the_recipe() {
        let mut catalog = scopes::catalog();
        let before = sub_agent_key_for(&catalog);
        catalog.push(ScopeRow { name: "tools:admin".into(), implied_by: vec!["mcp".into(), "tools:admin".into()] });
        let after = sub_agent_key_for(&catalog);
        assert_ne!(before.calls, after.calls);
        assert!(after.calls[1].arguments["scopes"].to_string().contains("tools:admin"));
        assert!(crate::llms_txt::render_recipe_lines(&after).contains("`tools:admin`"));
    }
}
