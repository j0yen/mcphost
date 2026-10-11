//! PRD-mcphost-call-cli-from-contract: `mcphost call <tool> --<arg> v`, whose
//! command table is the contract. The clap tree -- one subcommand per
//! canonical tool, one flag per `input_schema` property, its value parser,
//! `required` and help -- is built at startup from the embedded
//! `contracts/host-tools.v1.json`, so the shell cannot name a tool or an
//! argument the host does not serve. A hand-written table would be a second
//! copy of the contract; this module has none.

use std::sync::OnceLock;

use clap::{Arg, ArgAction, Command, builder::PossibleValuesParser};
use serde_json::Value;

/// The committed contract, embedded: `mcphost contract dump --check` keeps
/// this file byte-identical to the live tool registry.
pub const EMBEDDED_CONTRACT: &[u8] = include_bytes!("../contracts/host-tools.v1.json");

/// The one schema property that is never a flag: the tenant key travels as
/// the `Authorization: Bearer` header (`MCPHOST_KEY` / `--key-file`), never
/// as a command-line value that `ps` or shell history would keep.
pub const KEY_PROPERTY: &str = "tenant_key";

/// Ids of the flags `mcphost call` itself owns; a schema property of the same
/// name is renamed `--<name>-arg` (recorded as an override).
const RESERVED_IDS: &[&str] = &["url", "key_file", "json", "no_skew_check", "list", "completions", "help"];

const HELP_MAX_CHARS: usize = 120;

/// How a schema property becomes a flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropKind {
    Str,
    Int,
    Num,
    Bool,
    StrList,
    IntList,
    /// `object`, untyped, or anything else: `--<name>-json <JSON|@FILE|->`.
    Json,
}

#[derive(Debug, Clone)]
pub struct PropSpec {
    pub name: String,
    pub kind: PropKind,
    pub required: bool,
    pub description: String,
    pub enum_values: Vec<String>,
    /// The long flag, without the leading `--`.
    pub flag: String,
}

#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub props: Vec<PropSpec>,
}

#[derive(Debug, Clone)]
pub struct Contract {
    pub tools: Vec<ToolSpec>,
    /// Every place the generic rules did not apply, as `tool: why`.
    pub overrides: Vec<String>,
    pub sha: String,
    pub version: String,
}

/// First sentence of a description, whitespace-collapsed and cut to
/// [`HELP_MAX_CHARS`] -- the help text of a subcommand or flag.
pub fn first_sentence(description: &str) -> String {
    let flat = description.split_whitespace().collect::<Vec<_>>().join(" ");
    let sentence = flat.split(". ").next().unwrap_or_default().trim_end_matches('.');
    if sentence.chars().count() <= HELP_MAX_CHARS {
        sentence.to_string()
    } else {
        let cut: String = sentence.chars().take(HELP_MAX_CHARS - 1).collect();
        format!("{cut}…")
    }
}

fn kind_of(schema: &Value) -> PropKind {
    let types: Vec<&str> = match schema.get("type") {
        Some(Value::String(t)) => vec![t.as_str()],
        Some(Value::Array(ts)) => ts.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    let scalar = |t: &str| types.contains(&t);
    if scalar("array") {
        return match schema.get("items").and_then(|i| i.get("type")).and_then(Value::as_str) {
            Some("string") => PropKind::StrList,
            Some("integer") => PropKind::IntList,
            _ => PropKind::Json,
        };
    }
    if scalar("object") {
        PropKind::Json
    } else if scalar("string") {
        PropKind::Str
    } else if scalar("integer") {
        PropKind::Int
    } else if scalar("number") {
        PropKind::Num
    } else if scalar("boolean") {
        PropKind::Bool
    } else {
        PropKind::Json
    }
}

impl Contract {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let doc: Value = serde_json::from_slice(bytes).map_err(|e| format!("contract: {e}"))?;
        let entries = doc
            .get("tools")
            .and_then(Value::as_array)
            .ok_or("contract: no `tools` array")?;
        let mut tools = Vec::new();
        let mut overrides = Vec::new();
        for entry in entries {
            let name = entry.get("name").and_then(Value::as_str).ok_or("contract: tool without name")?;
            // Canonical = dotted and not a registered alias. The `host_x_y`
            // wire spellings and `host.tool_publish`-style aliases stay
            // accepted on input (see `resolve`) but own no subcommand.
            if !name.contains('.') || crate::tool_aliases::resolve(name).is_some() {
                continue;
            }
            let schema = entry.get("input_schema").cloned().unwrap_or(Value::Null);
            let required: Vec<&str> = schema
                .get("required")
                .and_then(Value::as_array)
                .map(|r| r.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let mut props = Vec::new();
            if let Some(map) = schema.get("properties").and_then(Value::as_object) {
                for (prop, pschema) in map {
                    if prop == KEY_PROPERTY {
                        continue;
                    }
                    let kind = kind_of(pschema);
                    let mut flag = prop.replace('_', "-");
                    if RESERVED_IDS.contains(&prop.as_str()) {
                        overrides.push(format!("{name}: property `{prop}` collides with a `mcphost call` flag, renamed --{flag}-arg"));
                        flag = format!("{flag}-arg");
                    }
                    if kind == PropKind::Json {
                        flag = format!("{flag}-json");
                    }
                    props.push(PropSpec {
                        name: prop.clone(),
                        kind,
                        required: required.contains(&prop.as_str()),
                        description: pschema
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        enum_values: pschema
                            .get("enum")
                            .and_then(Value::as_array)
                            .map(|e| e.iter().filter_map(Value::as_str).map(str::to_string).collect())
                            .unwrap_or_default(),
                        flag,
                    });
                }
            }
            tools.push(ToolSpec {
                name: name.to_string(),
                description: entry.get("description").and_then(Value::as_str).unwrap_or_default().to_string(),
                props,
            });
        }
        let sha = crate::api_contract::sha_hex(bytes);
        let version = format!("{}.{}", crate::api_contract::CONTRACT_VERSION, &sha[..12]);
        Ok(Self { tools, overrides, sha, version })
    }

    /// The canonical tool a user-typed name means: itself, a registered
    /// alias (`host.tool_publish`), or the `host_tool_publish` wire form.
    /// The bool is whether the name was rewritten.
    pub fn resolve(&self, typed: &str) -> Option<(&ToolSpec, bool)> {
        let find = |name: &str| self.tools.iter().find(|t| t.name == name);
        if let Some(tool) = find(typed) {
            return Some((tool, false));
        }
        if let Some(tool) = crate::tool_aliases::resolve(typed).and_then(find) {
            return Some((tool, true));
        }
        self.tools
            .iter()
            .find(|t| t.name.replace('.', "_") == typed)
            .map(|t| (t, true))
    }
}

/// The embedded contract, parsed once.
pub fn contract() -> &'static Contract {
    static CONTRACT: OnceLock<Contract> = OnceLock::new();
    CONTRACT.get_or_init(|| Contract::parse(EMBEDDED_CONTRACT).expect("embedded contract parses"))
}

fn arg_for(prop: &PropSpec) -> Arg {
    let mut arg = Arg::new(prop.name.clone())
        .long(prop.flag.clone())
        .required(prop.required)
        .help(first_sentence(&prop.description));
    arg = match prop.kind {
        PropKind::Str if !prop.enum_values.is_empty() => {
            arg.value_parser(PossibleValuesParser::new(prop.enum_values.clone()))
        }
        PropKind::Str => arg.value_parser(clap::value_parser!(String)),
        PropKind::Int => arg.value_parser(clap::value_parser!(i64)).allow_negative_numbers(true),
        PropKind::Num => arg.value_parser(clap::value_parser!(f64)).allow_negative_numbers(true),
        PropKind::Bool => arg
            .value_parser(clap::value_parser!(bool))
            .num_args(0..=1)
            .default_missing_value("true"),
        PropKind::StrList => arg.value_parser(clap::value_parser!(String)).action(ArgAction::Append),
        PropKind::IntList => arg
            .value_parser(clap::value_parser!(i64))
            .action(ArgAction::Append)
            .allow_negative_numbers(true),
        PropKind::Json => arg.value_parser(clap::value_parser!(String)).value_name("JSON|@FILE|-"),
    };
    arg
}

/// The `mcphost call` command tree for `contract`.
pub fn build_command(contract: &Contract) -> Command {
    let mut call = Command::new("call")
        .bin_name("mcphost call")
        .about("Call a host tool from the shell: one subcommand per tool, one flag per argument")
        .arg_required_else_help(true)
        .args_conflicts_with_subcommands(true)
        .arg(
            Arg::new("url")
                .long("url")
                .global(true)
                .env("MCPHOST_URL")
                .default_value("https://mcphost.dev")
                .help("Host base URL"),
        )
        .arg(
            Arg::new("key_file")
                .long("key-file")
                .global(true)
                .help("File holding the tenant key (otherwise $MCPHOST_KEY; never a flag value)"),
        )
        .arg(
            Arg::new("json")
                .long("json")
                .global(true)
                .action(ArgAction::SetTrue)
                .help("Print the result as one JSON line"),
        )
        .arg(
            Arg::new("no_skew_check")
                .long("no-skew-check")
                .global(true)
                .action(ArgAction::SetTrue)
                .help("Skip the contract-skew HEAD request"),
        )
        .arg(
            Arg::new("list")
                .long("list")
                .action(ArgAction::SetTrue)
                .help("List every tool, grouped by family"),
        )
        .arg(
            Arg::new("completions")
                .long("completions")
                .value_name("SHELL")
                .value_parser(["bash", "zsh", "fish"])
                .help("Print shell completions rendered from this same table"),
        );
    for tool in &contract.tools {
        let mut sub = Command::new(tool.name.clone()).about(first_sentence(&tool.description));
        for prop in &tool.props {
            sub = sub.arg(arg_for(prop));
        }
        call = call.subcommand(sub);
    }
    call
}

/// Flags of `mcphost call` that take a value, for locating the tool token.
const VALUE_FLAGS: &[&str] = &["--url", "--key-file", "--completions"];

/// Rewrite a typed alias (`host.tool_publish`, `host_tool_publish`) to its
/// canonical name so clap sees only canonical subcommands. Returns the
/// rewritten argv and, when a rewrite happened, `(typed, canonical)`.
pub fn rewrite_alias(contract: &Contract, args: &[String]) -> (Vec<String>, Option<AliasNote>) {
    let mut out = args.to_vec();
    let mut i = 0;
    while i < out.len() {
        let arg = out[i].as_str();
        if VALUE_FLAGS.contains(&arg) {
            i += 2;
            continue;
        }
        if arg.starts_with('-') {
            i += 1;
            continue;
        }
        if let Some((tool, true)) = contract.resolve(arg) {
            let note = (arg.to_string(), tool.name.clone());
            out[i] = tool.name.clone();
            return (out, Some(note));
        }
        break;
    }
    (out, None)
}

/// A typed alias and the canonical name it was rewritten to.
pub type AliasNote = (String, String);

/// Parse `args` (everything after `call`) against the contract's tree.
pub fn parse_args(
    contract: &Contract,
    args: &[String],
) -> Result<(clap::ArgMatches, Option<AliasNote>), clap::Error> {
    let (rewritten, note) = rewrite_alias(contract, args);
    let argv = std::iter::once("mcphost call".to_string()).chain(rewritten);
    build_command(contract).try_get_matches_from(argv).map(|m| (m, note))
}

/// A `--*-json` value: literal JSON, `@file` (read the file) or `-` (stdin).
fn read_json_value(flag: &str, raw: &str) -> Result<Value, String> {
    let text = if raw == "-" {
        let mut s = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)
            .map_err(|e| format!("--{flag}: reading stdin: {e}"))?;
        s
    } else if let Some(path) = raw.strip_prefix('@') {
        std::fs::read_to_string(path).map_err(|e| format!("--{flag}: reading {path}: {e}"))?
    } else {
        raw.to_string()
    };
    serde_json::from_str(&text).map_err(|e| format!("--{flag}: not valid JSON: {e}"))
}

/// The `tools/call` arguments object from the matched subcommand's flags.
pub fn arguments_from(tool: &ToolSpec, sub: &clap::ArgMatches) -> Result<Value, String> {
    let mut args = serde_json::Map::new();
    for prop in &tool.props {
        let id = prop.name.as_str();
        let value = match prop.kind {
            PropKind::Str => sub.get_one::<String>(id).map(|v| Value::String(v.clone())),
            PropKind::Int => sub.get_one::<i64>(id).map(|v| Value::from(*v)),
            PropKind::Num => sub.get_one::<f64>(id).and_then(|v| serde_json::Number::from_f64(*v)).map(Value::Number),
            PropKind::Bool => sub.get_one::<bool>(id).map(|v| Value::Bool(*v)),
            PropKind::StrList => sub
                .get_many::<String>(id)
                .map(|vs| Value::Array(vs.map(|v| Value::String(v.clone())).collect())),
            PropKind::IntList => sub.get_many::<i64>(id).map(|vs| Value::Array(vs.map(|v| Value::from(*v)).collect())),
            PropKind::Json => match sub.get_one::<String>(id) {
                Some(raw) => Some(read_json_value(&prop.flag, raw)?),
                None => None,
            },
        };
        if let Some(value) = value {
            args.insert(prop.name.clone(), value);
        }
    }
    Ok(Value::Object(args))
}

/// The tenant key: `$MCPHOST_KEY`, else the first line of `--key-file`.
/// Never a flag value, never printed.
fn load_key(key_file: Option<&String>) -> Result<Option<String>, String> {
    if let Ok(key) = std::env::var("MCPHOST_KEY")
        && !key.trim().is_empty()
    {
        return Ok(Some(key.trim().to_string()));
    }
    match key_file {
        Some(path) => {
            let text = std::fs::read_to_string(path).map_err(|e| format!("--key-file {path}: {e}"))?;
            Ok(Some(text.trim().to_string()).filter(|k| !k.is_empty()))
        }
        None => Ok(None),
    }
}

fn usage_failure(message: String) -> i32 {
    eprintln!("{}", serde_json::json!({"error_code": "args_invalid", "message": message, "data": null}));
    2
}

/// Tool family for `--list`: `host.tool.publish` -> `host.tool`, a
/// two-segment name is its own family stem (`host.redeem` -> `host`).
fn family(name: &str) -> String {
    let parts: Vec<&str> = name.split('.').collect();
    if parts.len() >= 3 { parts[..2].join(".") } else { parts[0].to_string() }
}

/// `mcphost call --list`: `name — description`, grouped by family.
pub fn render_list(contract: &Contract) -> String {
    let mut by_family: std::collections::BTreeMap<String, Vec<&ToolSpec>> = std::collections::BTreeMap::new();
    for tool in &contract.tools {
        by_family.entry(family(&tool.name)).or_default().push(tool);
    }
    let mut out = String::new();
    for (family, tools) in by_family {
        out.push_str(&format!("{family}\n"));
        for tool in tools {
            out.push_str(&format!("  {} — {}\n", tool.name, first_sentence(&tool.description)));
        }
    }
    out
}

fn sh_quote(text: &str) -> String {
    text.replace('\'', "'\\''")
}

fn zsh_spec_escape(text: &str) -> String {
    sh_quote(&text.replace('[', "\\[").replace(']', "\\]"))
}

/// Shell completions rendered from the same tree: tool names first, then
/// the chosen tool's flags. `shell` is `bash`, `zsh` or `fish`.
pub fn render_completions(contract: &Contract, shell: &str) -> String {
    let global_flags = [
        ("url", "Host base URL"),
        ("key-file", "File holding the tenant key"),
        ("json", "Print the result as one JSON line"),
        ("no-skew-check", "Skip the contract-skew HEAD request"),
    ];
    let mut out = String::new();
    match shell {
        "zsh" => {
            out.push_str("#compdef mcphost\n# generated by `mcphost call --completions zsh` from the embedded contract\n");
            out.push_str("_mcphost_call_tools() {\n  local -a tools\n  tools=(\n");
            for tool in &contract.tools {
                out.push_str(&format!(
                    "    '{}:{}'\n",
                    sh_quote(&tool.name.replace(':', "\\:")),
                    sh_quote(&first_sentence(&tool.description))
                ));
            }
            out.push_str("  )\n  _describe -t tools 'host tool' tools\n}\n");
            out.push_str("_mcphost_call() {\n  if [[ ${words[2]} != call ]]; then _default; return; fi\n");
            out.push_str("  if (( CURRENT == 3 )); then _mcphost_call_tools; return; fi\n  case ${words[3]} in\n");
            for tool in &contract.tools {
                let mut specs: Vec<String> = tool
                    .props
                    .iter()
                    .map(|prop| {
                        let action = if prop.enum_values.is_empty() {
                            String::new()
                        } else {
                            format!("({})", prop.enum_values.join(" "))
                        };
                        format!(
                            "'--{}=[{}]:{}:{}'",
                            prop.flag,
                            zsh_spec_escape(&first_sentence(&prop.description)),
                            prop.name,
                            action
                        )
                    })
                    .collect();
                specs.extend(
                    global_flags
                        .iter()
                        .map(|(flag, help)| format!("'--{flag}[{}]'", zsh_spec_escape(help))),
                );
                out.push_str(&format!("    {}) _arguments {} ;;\n", tool.name, specs.join(" \\\n      ")));
            }
            out.push_str("  esac\n}\ncompdef _mcphost_call mcphost\n");
        }
        "bash" => {
            out.push_str("# generated by `mcphost call --completions bash` from the embedded contract\n_mcphost_call() {\n");
            out.push_str("  local cur=${COMP_WORDS[COMP_CWORD]}\n  [[ ${COMP_WORDS[1]} == call ]] || return 0\n");
            let names: Vec<&str> = contract.tools.iter().map(|t| t.name.as_str()).collect();
            out.push_str(&format!(
                "  if (( COMP_CWORD == 2 )); then COMPREPLY=($(compgen -W '{}' -- \"$cur\")); return 0; fi\n  case ${{COMP_WORDS[2]}} in\n",
                names.join(" ")
            ));
            for tool in &contract.tools {
                let mut flags: Vec<String> = tool.props.iter().map(|p| format!("--{}", p.flag)).collect();
                flags.extend(global_flags.iter().map(|(f, _)| format!("--{f}")));
                out.push_str(&format!(
                    "    {}) COMPREPLY=($(compgen -W '{}' -- \"$cur\")) ;;\n",
                    tool.name,
                    flags.join(" ")
                ));
            }
            out.push_str("  esac\n}\ncomplete -F _mcphost_call mcphost\n");
        }
        _ => {
            out.push_str("# generated by `mcphost call --completions fish` from the embedded contract\n");
            out.push_str("function __mcphost_call_no_tool\n  set -l t (commandline -opc)\n  test (count $t) -eq 2; and test $t[2] = call\nend\n");
            out.push_str("function __mcphost_call_tool\n  set -l t (commandline -opc)\n  test (count $t) -ge 3; and test $t[2] = call; and test $t[3] = $argv[1]\nend\n");
            for tool in &contract.tools {
                out.push_str(&format!(
                    "complete -c mcphost -n __mcphost_call_no_tool -a '{}' -d '{}'\n",
                    tool.name,
                    sh_quote(&first_sentence(&tool.description))
                ));
                for prop in &tool.props {
                    out.push_str(&format!(
                        "complete -c mcphost -n '__mcphost_call_tool {}' -l {} -d '{}'\n",
                        tool.name,
                        prop.flag,
                        sh_quote(&first_sentence(&prop.description))
                    ));
                }
            }
        }
    }
    out
}

/// How long the skew check may delay a call.
const SKEW_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// The host's contract sha from one `HEAD <url>/contract.json` (the
/// `ETag` is the quoted sha). `None` on any failure: the check is advisory.
async fn host_contract_sha(url: &str) -> Option<String> {
    let client = reqwest::Client::builder().timeout(SKEW_TIMEOUT).build().ok()?;
    let resp = client.head(format!("{url}/contract.json")).send().await.ok()?;
    let etag = resp.headers().get(reqwest::header::ETAG)?.to_str().ok()?;
    let sha = etag.trim_start_matches("W/").trim_matches('"');
    (!sha.is_empty()).then(|| sha.to_string())
}

/// `mcphost call ...` (`args` = everything after `call`); returns the
/// process exit code. A clap usage error exits 2 before any network.
pub async fn run(args: &[String]) -> i32 {
    let contract = contract();
    let (matches, alias_note) = match parse_args(contract, args) {
        Ok(parsed) => parsed,
        Err(e) => e.exit(),
    };
    if let Some(shell) = matches.get_one::<String>("completions") {
        print!("{}", render_completions(contract, shell));
        return 0;
    }
    if matches.get_flag("list") {
        print!("{}", render_list(contract));
        return 0;
    }
    let Some((tool_name, sub)) = matches.subcommand() else {
        return usage_failure("name a tool, or pass --list / --completions <shell>".to_string());
    };
    let Some((tool, _)) = contract.resolve(tool_name) else {
        return usage_failure(format!("unknown tool {tool_name}"));
    };
    let arguments = match arguments_from(tool, sub) {
        Ok(a) => a,
        Err(message) => return usage_failure(message),
    };
    let key = match load_key(matches.get_one::<String>("key_file")) {
        Ok(k) => k,
        Err(message) => return usage_failure(message),
    };
    let url = matches.get_one::<String>("url").cloned().unwrap_or_default();
    let url = url.trim_end_matches('/');
    if !matches.get_flag("no_skew_check")
        && let Some(host_sha) = host_contract_sha(url).await
        && host_sha != contract.sha
    {
        let short = host_sha.get(..12).unwrap_or(&host_sha);
        eprintln!(
            "contract skew: binary {} host {}.{short}",
            contract.version,
            crate::api_contract::CONTRACT_VERSION
        );
    }
    if let Some((typed, canonical)) = alias_note {
        eprintln!("note: {typed} is an alias; calling {canonical}");
    }

    let client = reqwest::Client::new();
    let outcome = crate::client::rpc_call(
        &client,
        url,
        crate::client::RpcRequest {
            method: "tools/call",
            params: serde_json::json!({"name": tool.name, "arguments": arguments}),
            id: 1,
            bearer: key.as_deref(),
        },
    )
    .await;
    match outcome {
        Ok(result) => {
            let structured = crate::client::extract_structured(&result);
            let rendered = if matches.get_flag("json") {
                serde_json::to_string(&structured)
            } else {
                serde_json::to_string_pretty(&structured)
            };
            println!("{}", rendered.unwrap_or_default());
            0
        }
        Err(failure) => {
            let error = crate::client::classify(&failure);
            eprintln!("{}", error.envelope());
            error.exit
        }
    }
}
