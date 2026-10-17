//! PRD-mcphost-prompts-from-published-tools: `prompts/list` and
//! `prompts/get`. The recipe prompts are generated from [`RECIPES`], the one
//! table `host.quickstart {kind}` also reads for a recipe's walk-through
//! text, so the tool and the prompt cannot drift. Rendering is a pure
//! substitution of `{{slot}}`s: no provider call, no metering.

use rmcp::model::{GetPromptResult, Prompt, PromptArgument, PromptMessage, Role};
use serde_json::{Map, Value};

/// The registered kind whose tools are published as prompts.
pub const PROMPT_KIND: &str = "prompt";

/// Prefix a recipe's prompt name takes when a tenant tool already owns the
/// plain name (the tool wins).
pub const RECIPE_PREFIX: &str = "recipe_";

/// One shipped quickstart recipe. `arguments` is `(name, description,
/// required)`; `body` carries a `{{name}}` slot per argument.
pub struct Recipe {
    pub name: &'static str,
    pub description: &'static str,
    pub arguments: &'static [(&'static str, &'static str, bool)],
    pub body: &'static str,
}

/// Every shipped recipe. Adding or editing a row changes `prompts/list`,
/// `prompts/get`, `host.quickstart {kind}` and `docs/protocol/prompts.md`
/// together.
pub const RECIPES: &[Recipe] = &[
    Recipe {
        name: "webhook-inbox",
        description: "Receive signed webhooks from an outside service in one call, then read what arrived.",
        arguments: &[
            ("name", "Name for the inbox (becomes the tool name), e.g. stripe-events", true),
            ("sender", "The service that will POST to the inbox, e.g. GitHub or Stripe", false),
        ],
        body: "Set up a webhook inbox named {{name}} on mcphost for events from {{sender}}.\n\n\
1. Call host.webhook_inbox.create with {\"name\": \"{{name}}\"}. It returns url, secret and wait (the host.msg.wait arguments) in one call; repeating the exact call is safe and returns the same url with the secret omitted unless rotate is true.\n\
2. Give the sender the url and the secret, and have it POST its body signed with the secret.\n\
3. Read what arrived with host.tool_call(name={{name}}) or host.msg.wait(**wait.args).\n\n\
Report the url, a redacted secret, and the first message that arrived.",
    },
    Recipe {
        name: "schedules",
        description: "Run a published tool on a cron schedule and fire it once to prove it works.",
        arguments: &[
            ("tool", "Name of the tool to run on the schedule, e.g. nightly_report", true),
            ("schedule", "Five-field cron expression, e.g. */5 * * * *", true),
        ],
        body: "Put the tool {{tool}} on the schedule {{schedule}} using mcphost.\n\n\
1. Publish the tool with host.tool_publish {\"name\": \"{{tool}}\", \"kind\": \"http\", \"spec\": {...}} if it does not exist yet.\n\
2. Call host.trigger.set {\"tool\": \"{{tool}}\", \"kind\": \"schedule\", \"name\": \"{{tool}}\", \"schedule\": \"{{schedule}}\"}. The name is the trigger's identity: repeating the call updates the same trigger in place instead of minting a duplicate.\n\
3. Prove it with host.trigger.fire using the id host.trigger.set returned, then read the run result.\n\n\
Report the trigger id and the result of the fired run.",
    },
    Recipe {
        name: "docs-qa",
        description: "Store documents on the host and answer questions from them with cited passages.",
        arguments: &[
            ("document", "Name of the document to store, e.g. notes.md", true),
            ("question", "The question to answer from the stored documents", true),
        ],
        body: "Answer a question from documents held on mcphost.\n\n\
1. Store the document with host.docs.put {\"name\": \"{{document}}\", \"content\": \"<the document text>\"}.\n\
2. Search it with host.docs.search for the question: {{question}}\n\
3. Answer using only the passages host.docs.search returned and cite the document name for each claim.\n\n\
If no passage answers the question, say so instead of guessing.",
    },
];

/// Look a recipe up by prompt name.
pub fn find_recipe(name: &str) -> Option<&'static Recipe> {
    RECIPES.iter().find(|r| r.name == name)
}

/// The walk-through text of `recipe` for `host.quickstart`: the body with
/// each slot shown as `<name>`.
pub fn recipe_text(name: &str) -> Option<String> {
    let recipe = find_recipe(name)?;
    let mut text = recipe.body.to_string();
    for (arg, _, _) in recipe.arguments {
        text = text.replace(&format!("{{{{{arg}}}}}"), &format!("<{arg}>"));
    }
    Some(text)
}

/// `PromptArgument`s from a JSON-Schema object: one per `properties` key,
/// `required` from the schema's `required` array, the description carrying
/// the JSON type (`"text (string, required)"`) because the spec's
/// `PromptArgument` has no type field.
pub fn prompt_arguments_from_schema(schema: &Value) -> Vec<PromptArgument> {
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let Some(props) = schema.get("properties").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut out: Vec<PromptArgument> = props
        .iter()
        .map(|(name, prop)| {
            let is_required = required.contains(&name.as_str());
            let ty = prop.get("type").and_then(Value::as_str).unwrap_or("any");
            let flag = if is_required { "required" } else { "optional" };
            let detail = prop.get("description").and_then(Value::as_str);
            let description = match detail {
                Some(d) => format!("{name} ({ty}, {flag}): {d}"),
                None => format!("{name} ({ty}, {flag})"),
            };
            PromptArgument::new(name.clone()).with_description(description).with_required(is_required)
        })
        .collect();
    // Required arguments first, then the schema's own (sorted) key order.
    out.sort_by_key(|a| !a.required.unwrap_or(false));
    out
}

/// The recipe prompts, `RECIPES` order.
pub fn recipe_prompts() -> Vec<Prompt> {
    RECIPES
        .iter()
        .map(|r| {
            let args = r
                .arguments
                .iter()
                .map(|(name, desc, required)| PromptArgument::new(*name).with_description(*desc).with_required(*required))
                .collect();
            Prompt::new(r.name, Some(r.description), Some(args))
        })
        .collect()
}

/// The recipe prompts as a tenant sees them: `taken` (the tenant's prompt
/// tool names) wins a name collision and the recipe is listed under
/// [`RECIPE_PREFIX`] + its name.
pub fn recipe_prompts_avoiding(taken: &[String]) -> Vec<Prompt> {
    recipe_prompts()
        .into_iter()
        .map(|mut p| {
            if taken.contains(&p.name) {
                p.name = format!("{RECIPE_PREFIX}{}", p.name);
            }
            p
        })
        .collect()
}

/// Resolve a recipe prompt name as listed by [`recipe_prompts_avoiding`]:
/// the plain name, or the prefixed name when `taken` shadows the plain one.
pub fn recipe_name_for<'a>(requested: &'a str, taken: &[String]) -> &'a str {
    match requested.strip_prefix(RECIPE_PREFIX) {
        Some(plain) if taken.iter().any(|t| t == plain) && find_recipe(plain).is_some() => plain,
        _ => requested,
    }
}

pub const PROMPTS_DOC_START: &str = "<!-- prompts:start -->";
pub const PROMPTS_DOC_END: &str = "<!-- prompts:end -->";

/// The whole of `docs/protocol/prompts.md`: the recipe table rendered from
/// [`recipe_prompts`], the same list `prompts/list` answers keyless.
/// `scripts/sandbox-api-doc-check.sh` passes only while the file equals this.
pub fn render_prompts_doc() -> String {
    let mut out = String::from("# MCP prompts\n\n");
    out.push_str(PROMPTS_DOC_START);
    out.push_str("\n## Recipe prompts\n\n");
    out.push_str(
        "Generated by `mcphost prompts-section` from `RECIPES` (`src/prompts.rs`) -- the table \
         `prompts/list` and `host.quickstart {kind}` also read; do not hand-edit. A keyed tenant's \
         `prompt`-kind tools are listed beside these.\n\n",
    );
    out.push_str("| prompt | description | arguments |\n|---|---|---|\n");
    for p in recipe_prompts() {
        let args = p
            .arguments
            .iter()
            .flatten()
            .map(|a| {
                format!(
                    "`{}` ({}): {}",
                    a.name,
                    if a.required == Some(true) { "required" } else { "optional" },
                    a.description.as_deref().unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join("<br>");
        out.push_str(&format!("| `{}` | {} | {} |\n", p.name, p.description.as_deref().unwrap_or_default(), args));
    }
    out.push_str(PROMPTS_DOC_END);
    out.push('\n');
    out
}

/// Whether a successful `host.tool.publish` / `host.tool.remove` changed the
/// prompt list: the tool's kind before the call or the kind published is
/// `prompt`. A dry-run publish changes nothing.
pub fn changes_prompt_list(tool_name: &str, args: &Value, kind_before: Option<&str>, result: &Value) -> bool {
    match tool_name {
        "host.tool.publish" => {
            args.get("dry_run").and_then(Value::as_bool) != Some(true)
                && result.get("ok").and_then(Value::as_bool) != Some(false)
                && (kind_before == Some(PROMPT_KIND) || args.get("kind").and_then(Value::as_str) == Some(PROMPT_KIND))
        }
        "host.tool.remove" => result.get("removed").is_some() && kind_before == Some(PROMPT_KIND),
        _ => false,
    }
}

/// A `prompts/get` failure, mapped to `-32602` by the handler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptError {
    NotFound(String),
    MissingArgument { prompt: String, argument: String },
}

impl PromptError {
    pub fn into_mcp(self) -> rmcp::ErrorData {
        match self {
            PromptError::NotFound(name) => rmcp::ErrorData::invalid_params(
                format!("prompt_not_found: no prompt named `{name}`; call prompts/list"),
                Some(serde_json::json!({"code": "prompt_not_found", "name": name})),
            ),
            PromptError::MissingArgument { prompt, argument } => rmcp::ErrorData::invalid_params(
                format!("prompt `{prompt}` requires the argument `{argument}`"),
                Some(serde_json::json!({"code": "missing_argument", "prompt": prompt, "argument": argument})),
            ),
        }
    }
}

/// Replace every `{{name}}` slot in `template` with the argument of that
/// name; an absent argument leaves an empty string.
pub fn substitute(template: &str, args: &Map<String, Value>, names: &[&str]) -> String {
    let mut text = template.to_string();
    for name in names {
        let value = match args.get(*name) {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Null) | None => String::new(),
            Some(other) => other.to_string(),
        };
        text = text.replace(&format!("{{{{{name}}}}}"), &value);
    }
    text
}

/// Render one recipe: required arguments checked in declaration order.
pub fn render_recipe(name: &str, args: &Map<String, Value>) -> Result<GetPromptResult, PromptError> {
    let recipe = find_recipe(name).ok_or_else(|| PromptError::NotFound(name.to_string()))?;
    for (arg, _, required) in recipe.arguments {
        let present = matches!(args.get(*arg), Some(v) if !v.is_null() && v.as_str() != Some(""));
        if *required && !present {
            return Err(PromptError::MissingArgument { prompt: name.to_string(), argument: (*arg).to_string() });
        }
    }
    let names: Vec<&str> = recipe.arguments.iter().map(|(n, _, _)| *n).collect();
    let text = substitute(recipe.body, args, &names);
    Ok(GetPromptResult::new(vec![PromptMessage::new_text(Role::User, text)]).with_description(recipe.description))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_slot_in_a_body_is_a_declared_argument() {
        for r in RECIPES {
            let mut rest = r.body;
            while let Some(i) = rest.find("{{") {
                let end = rest[i..].find("}}").expect("closing braces") + i;
                let slot = &rest[i + 2..end];
                assert!(r.arguments.iter().any(|(n, _, _)| *n == slot), "{}: undeclared slot {slot}", r.name);
                rest = &rest[end + 2..];
            }
        }
    }
}
