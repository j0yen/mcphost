//! PRD-mcphost-tool-reach-manifest-v2: what a tool can touch -- hosts,
//! secrets, `mcphost.state` keys, bridge modules, tools it calls, network
//! mode -- derived from the stored `kind` and `spec` alone, on read. Nothing
//! is stored and nothing runs, so a manifest cannot disagree with its spec.
//! The scan is static and best effort: whatever it cannot read literally is
//! reported as [`DYNAMIC`], so it over-reports and never under-reports.

use std::collections::BTreeSet;

use serde_json::{Value, json};

use crate::kinds::python::{BRIDGE_ATTRS, BRIDGE_MODULES};

/// The marker for a value built at run time (a templated host, a non-literal
/// state key, a name reached through an alias).
pub const DYNAMIC: &str = "*dynamic*";

/// How many chain-in-chain hops [`derive_with_steps`] follows before it stops
/// descending (a chain naming itself, directly or through another, ends here).
pub const MAX_CHAIN_DEPTH: usize = 4;

// ---- the manifest -----------------------------------------------------------

/// The `mcphost.state` keys and tables a tool names.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StateReach {
    pub reads: BTreeSet<String>,
    pub writes: BTreeSet<String>,
    pub tables: BTreeSet<String>,
}

/// What one tool can touch. Every list is sorted and de-duplicated, so the
/// same spec always yields the same bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reach {
    /// Literal hosts only; a templated host sets `dynamic_hosts` instead.
    pub hosts: BTreeSet<String>,
    pub dynamic_hosts: bool,
    /// Secret names (`vault:<provider>` for a vault-backed upstream). Names only.
    pub secrets: BTreeSet<String>,
    pub state: StateReach,
    /// Every `mcphost.<name>` the source names.
    pub modules: BTreeSet<String>,
    /// Tools called: a chain's step tools, or `mcphost.call`'s first argument.
    pub calls: BTreeSet<String>,
    /// The spec's sandbox network mode: `none`, `egress` or `public`.
    pub network: String,
}

impl Default for Reach {
    fn default() -> Self {
        Reach {
            hosts: BTreeSet::new(),
            dynamic_hosts: false,
            secrets: BTreeSet::new(),
            state: StateReach::default(),
            modules: BTreeSet::new(),
            calls: BTreeSet::new(),
            network: "none".to_string(),
        }
    }
}

fn list(set: &BTreeSet<String>) -> Value {
    Value::Array(set.iter().map(|s| Value::String(s.clone())).collect())
}

impl Reach {
    /// The owner's full view.
    pub fn to_json(&self) -> Value {
        json!({
            "hosts": list(&self.hosts),
            "dynamic_hosts": self.dynamic_hosts,
            "secrets": list(&self.secrets),
            "state": {
                "reads": list(&self.state.reads),
                "writes": list(&self.state.writes),
                "tables": list(&self.state.tables),
            },
            "modules": list(&self.modules),
            "calls": list(&self.calls),
            "network": self.network,
        })
    }

    /// What a sharee or a scoped sub-key may see: where the tool goes and how
    /// many secrets it holds, never a secret name, a state key or a table.
    pub fn caller_view(&self) -> Value {
        json!({
            "hosts": list(&self.hosts),
            "dynamic_hosts": self.dynamic_hosts,
            "network": self.network,
            "secrets_count": self.secrets.len(),
        })
    }

    /// Folds a chain step's reach into this one. `calls` is left alone: a
    /// chain's `calls` is the step names its own spec gives.
    fn absorb(&mut self, step: Reach) {
        self.hosts.extend(step.hosts);
        self.dynamic_hosts |= step.dynamic_hosts;
        self.secrets.extend(step.secrets);
        self.state.reads.extend(step.state.reads);
        self.state.writes.extend(step.state.writes);
        self.state.tables.extend(step.state.tables);
        self.modules.extend(step.modules);
        if network_rank(&step.network) > network_rank(&self.network) {
            self.network = step.network;
        }
    }
}

fn network_rank(mode: &str) -> u8 {
    match mode {
        "public" => 2,
        "egress" => 1,
        _ => 0,
    }
}

// ---- derivation -------------------------------------------------------------

/// A chain step's local tool name to its stored `(kind, spec)`, when the chain's
/// owner holds it.
pub type StepResolver<'a> = dyn Fn(&str) -> Option<(String, Value)> + 'a;

/// A pure function of a tool's stored `kind` and `spec`. A chain's steps
/// contribute their names to `calls` and nothing else here; the host fills in
/// the steps its own tenant holds through [`derive_with_steps`].
pub fn derive(kind: &str, spec: &Value) -> Reach {
    derive_with_steps(kind, spec, &|_| None)
}

/// [`derive`], with `steps` resolving a chain step's local tool name to its
/// stored `(kind, spec)`. The resolver answers for the chain owner's own
/// tenant only: a `<ns>.<name>` step names a shared-in tool and is never
/// resolved, so another tenant's secret names and state keys cannot enter
/// this manifest.
pub fn derive_with_steps(kind: &str, spec: &Value, steps: &StepResolver) -> Reach {
    derive_at(kind, spec, steps, 0)
}

fn derive_at(kind: &str, spec: &Value, steps: &StepResolver, depth: usize) -> Reach {
    match kind {
        "http" => derive_http(spec),
        "chain" => derive_chain(spec, steps, depth),
        "python" | "js" => derive_sandboxed(spec),
        // The internal `uptime` kind keeps its probe urls in a tenant table,
        // not in its spec, so the spec cannot name them.
        "uptime" => Reach { dynamic_hosts: true, ..Reach::default() },
        // wasm, jq and echo have no network field and no bridge: empty.
        _ => Reach::default(),
    }
}

fn derive_http(spec: &Value) -> Reach {
    let mut reach = Reach::default();
    match crate::kinds::http::reach_facts(spec) {
        Some(facts) => {
            match facts.host {
                Some(host) => {
                    reach.hosts.insert(host);
                }
                None => reach.dynamic_hosts = true,
            }
            reach.secrets = facts.secrets;
            if let Some(provider) = facts.vault_provider {
                reach.secrets.insert(format!("vault:{provider}"));
            }
        }
        // A stored spec that no longer parses: say the host is unknown.
        None => reach.dynamic_hosts = true,
    }
    reach
}

/// The tool name each step of a chain spec gives, in order.
fn chain_step_names(spec: &Value) -> impl Iterator<Item = &str> {
    spec.get("steps")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|step| step.get("tool").and_then(Value::as_str))
}

/// The steps of a chain spec that name the owner's own tools (no dot): the
/// ones [`derive_with_steps`] can resolve.
pub fn chain_local_steps(spec: &Value) -> Vec<String> {
    chain_step_names(spec).filter(|n| !n.contains('.')).map(str::to_string).collect()
}

fn derive_chain(spec: &Value, steps: &StepResolver, depth: usize) -> Reach {
    let mut reach = Reach::default();
    for name in chain_step_names(spec) {
        reach.calls.insert(name.to_string());
        // A name with a dot is a shared-in tool: its name only.
        if name.contains('.') || depth >= MAX_CHAIN_DEPTH {
            continue;
        }
        if let Some((kind, step_spec)) = steps(name) {
            reach.absorb(derive_at(&kind, &step_spec, steps, depth + 1));
        }
    }
    reach
}

fn derive_sandboxed(spec: &Value) -> Reach {
    let mut reach = Reach::default();
    let network = spec.get("network").and_then(Value::as_str).unwrap_or("none");
    reach.network = network.to_string();
    // The sandbox may open any socket once it has a network grant.
    reach.dynamic_hosts = network != "none";
    if let Some(secrets) = spec.get("secrets").and_then(Value::as_array) {
        reach.secrets.extend(secrets.iter().filter_map(Value::as_str).map(str::to_string));
    }
    if let Some(source) = spec.get("source").and_then(Value::as_str) {
        scan_source(source, &mut reach);
    }
    reach
}

// ---- the source scan ---------------------------------------------------------

/// Which `mcphost.state.<verb>` fills which list.
#[derive(Clone, Copy)]
enum StateList {
    Reads,
    Writes,
    Tables,
}

/// `mcphost.state.<verb>` -> the list its first argument belongs to. Every
/// `state` signature in [`BRIDGE_MODULES`] has a row here
/// (`trm_ac06_*` checks).
pub const STATE_VERBS: &[(&str, &str)] = &[
    ("get", "reads"),
    ("list", "reads"),
    ("set", "writes"),
    ("delete", "writes"),
    ("table_create", "tables"),
    ("table_drop", "tables"),
    ("insert", "tables"),
    ("query", "tables"),
    ("delete_rows", "tables"),
];

fn state_list(verb: &str) -> Option<StateList> {
    STATE_VERBS.iter().find(|(v, _)| *v == verb).map(|(_, list)| match *list {
        "reads" => StateList::Reads,
        "writes" => StateList::Writes,
        _ => StateList::Tables,
    })
}

/// How a bridge name is reached, by the one table [`REACH_CATEGORIES`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReachRule {
    /// `mcphost.state`: the first argument of each verb is a key or a table.
    StateKeys,
    /// `mcphost.call`: the first argument is the tool called.
    Calls,
    /// Every other bridge name: the manifest names the module, not what it touches.
    ModulesOnly,
}

/// One reach rule per bridge name: every [`BRIDGE_MODULES`] name and every
/// [`BRIDGE_ATTRS`] attribute (`call`, `progress`, `quota`). A name added to
/// either set without a row here fails `trm_ac06_*`, which names it.
pub const REACH_CATEGORIES: &[(&str, ReachRule)] = &[
    ("state", ReachRule::StateKeys),
    ("table", ReachRule::ModulesOnly),
    ("docs", ReachRule::ModulesOnly),
    ("index", ReachRule::ModulesOnly),
    ("blob", ReachRule::ModulesOnly),
    ("lineage", ReachRule::ModulesOnly),
    ("drift", ReachRule::ModulesOnly),
    ("channel", ReachRule::ModulesOnly),
    ("msg", ReachRule::ModulesOnly),
    ("sampling", ReachRule::ModulesOnly),
    ("email", ReachRule::ModulesOnly),
    ("elicit", ReachRule::ModulesOnly),
    ("call", ReachRule::Calls),
    ("progress", ReachRule::ModulesOnly),
    ("quota", ReachRule::ModulesOnly),
];

/// The rule for a bridge name, if it has one.
pub fn rule_for(name: &str) -> Option<ReachRule> {
    REACH_CATEGORIES.iter().find(|(n, _)| *n == name).map(|(_, r)| *r)
}

/// The attribute name of a [`BRIDGE_ATTRS`] signature (`mcphost.call(...) -- ...`).
fn attr_name(signature: &str) -> Option<&str> {
    let head = signature.split(['(', ' ']).next()?;
    head.strip_prefix("mcphost.")
}

/// The bridge names in `modules` (module names) and `attrs` (attribute
/// signatures such as `mcphost.call(name, ...) -- ...`) that [`REACH_CATEGORIES`]
/// has no row for. An attribute whose signature does not parse is named whole.
pub fn uncovered_names<'a>(modules: &[&'a str], attrs: &[&'a str]) -> Vec<&'a str> {
    let attr_names = attrs.iter().map(|a| attr_name(a).unwrap_or(a));
    modules.iter().copied().chain(attr_names).filter(|name| rule_for(name).is_none()).collect()
}

/// [`uncovered_names`] over the live [`BRIDGE_MODULES`] and [`BRIDGE_ATTRS`].
pub fn uncovered_bridge_names() -> Vec<&'static str> {
    let modules: Vec<&str> = BRIDGE_MODULES.iter().map(|m| m.name).collect();
    uncovered_names(&modules, BRIDGE_ATTRS)
}

/// The `mcphost.state.<verb>` verbs `signatures` name that [`STATE_VERBS`] has no
/// row for.
pub fn uncovered_state_verbs<'a>(signatures: &[&'a str]) -> Vec<&'a str> {
    signatures
        .iter()
        .filter_map(|sig| sig.strip_prefix("mcphost.state."))
        .map(leading_ident)
        .filter(|verb| state_list(verb).is_none())
        .collect()
}

/// Names the bridge hangs off `mcphost` that name no reach: the error classes
/// a tool catches.
fn is_error_class(name: &str) -> bool {
    matches!(name, "CallError" | "BridgeError" | "BridgeShapeError" | "SamplingError")
}

fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn leading_ident(s: &str) -> &str {
    let end = s.find(|c: char| !is_ident(c)).unwrap_or(s.len());
    &s[..end]
}

/// An unreadable `mcphost` mention: it may touch any key or table.
fn mark_dynamic(reach: &mut Reach) {
    reach.state.reads.insert(DYNAMIC.to_string());
    reach.state.writes.insert(DYNAMIC.to_string());
    reach.state.tables.insert(DYNAMIC.to_string());
}

/// The scan itself: every spelling the table allows is read exactly; any other
/// line that names `mcphost` marks `state` dynamic.
fn scan_source(source: &str, reach: &mut Reach) {
    let mut offset = 0usize;
    for raw_line in source.split_inclusive('\n') {
        let line_start = offset;
        offset += raw_line.len();
        let trimmed = raw_line.trim_start();
        if trimmed.starts_with('#') || trimmed.starts_with("//") {
            continue;
        }
        let mut from = 0usize;
        while let Some(found) = raw_line[from..].find("mcphost") {
            let idx = from + found;
            let end = idx + "mcphost".len();
            from = end;
            let before_char = raw_line[..idx].chars().next_back();
            let after_char = raw_line[end..].chars().next();
            // `mcphost_sdk`, `my_mcphost`, `mcphost2`: some other name.
            if before_char.is_some_and(is_ident) || after_char.is_some_and(is_ident) {
                continue;
            }
            let before = raw_line[..idx].trim();
            let rest_of_source = &source[line_start + end..];
            scan_mention(trimmed, before, rest_of_source, reach);
        }
    }
}

/// One `mcphost` token. `line` is the trimmed line it sits on, `before` the
/// line text left of it, `after` the whole source right of it.
fn scan_mention(line: &str, before: &str, after: &str, reach: &mut Reach) {
    let in_import = line.starts_with("import ") && (before.ends_with("import") || before.ends_with(','));
    let in_from = line.starts_with("from ") && before == "from";
    if in_from {
        // `from mcphost import state` / `from mcphost.state import get`: the
        // names are recorded, and the bare names that follow carry no call
        // shape, so the state lists go dynamic.
        scan_from_import(after, reach);
        mark_dynamic(reach);
        return;
    }
    let Some(dotted) = after.strip_prefix('.') else {
        // A bare `mcphost`: only a plain `import mcphost` is clean.
        let tail = after.trim_start_matches([' ', '\t']);
        let clean = in_import && (tail.is_empty() || tail.starts_with([';', ',', '#', '\n', '\r']));
        if !clean {
            mark_dynamic(reach);
        }
        return;
    };
    let name = leading_ident(dotted);
    if name.is_empty() {
        mark_dynamic(reach);
        return;
    }
    if is_error_class(name) {
        return;
    }
    let Some(rule) = rule_for(name) else {
        // An unknown `mcphost.<name>` is refused at publish; if one is stored,
        // be conservative.
        mark_dynamic(reach);
        return;
    };
    reach.modules.insert(name.to_string());
    let after_name = &dotted[name.len()..];
    if in_import {
        // `import mcphost.state` is clean; `import mcphost.state as s` is an alias.
        if after_name.trim_start().starts_with("as ") {
            mark_dynamic(reach);
        }
        return;
    }
    match rule {
        ReachRule::StateKeys => match after_name.strip_prefix('.') {
            Some(verb_rest) => {
                let verb = leading_ident(verb_rest);
                let args = verb_rest[verb.len()..].trim_start().strip_prefix('(');
                match (args, state_list(verb)) {
                    (Some(args), Some(which)) => {
                        let literal = first_literal_arg(args);
                        let target = match which {
                            StateList::Reads => &mut reach.state.reads,
                            StateList::Writes => &mut reach.state.writes,
                            StateList::Tables => &mut reach.state.tables,
                        };
                        target.insert(literal.unwrap_or_else(|| DYNAMIC.to_string()));
                    }
                    // `mcphost.state.x` with no call, or a verb the table lacks.
                    _ => mark_dynamic(reach),
                }
            }
            // `s = mcphost.state`: the alias reaches anything.
            None => mark_dynamic(reach),
        },
        ReachRule::Calls => match after_name.trim_start().strip_prefix('(') {
            Some(args) => {
                reach.calls.insert(first_literal_arg(args).unwrap_or_else(|| DYNAMIC.to_string()));
            }
            None => mark_dynamic(reach),
        },
        ReachRule::ModulesOnly => {
            // `mcphost.progress(`, `mcphost.elicit(` (callable) or
            // `mcphost.<module>.<verb>(`; anything else is an alias.
            let called = after_name.trim_start().starts_with('(');
            let verb_call = after_name
                .strip_prefix('.')
                .is_some_and(|v| v[leading_ident(v).len()..].trim_start().starts_with('('));
            if !called && !verb_call {
                mark_dynamic(reach);
            }
        }
    }
}

/// `from mcphost[.<module>] import a, b as c`: records the modules named.
fn scan_from_import(after: &str, reach: &mut Reach) {
    let line = after.lines().next().unwrap_or("");
    let (module_part, names_part) = match line.split_once(" import ") {
        Some((m, n)) => (m, n),
        None => (line, ""),
    };
    if let Some(module) = module_part.trim().strip_prefix('.') {
        let module = leading_ident(module);
        if rule_for(module).is_some() {
            reach.modules.insert(module.to_string());
        }
    }
    for part in names_part.split(',') {
        let name = leading_ident(part.trim().trim_start_matches('('));
        if rule_for(name).is_some() {
            reach.modules.insert(name.to_string());
        }
    }
}

/// The first positional argument of a call, when it is one plain string
/// literal; `args` is the source right after the `(`. A keyword spelling of
/// the first parameter (`key=`, `prefix=`, `name=`, `table=`) counts. `None`
/// for anything built at run time: concatenation, an f-string, a variable, an
/// escape, no argument at all.
fn first_literal_arg(args: &str) -> Option<String> {
    let mut rest = args.trim_start();
    let ident = leading_ident(rest);
    let after_ident = rest[ident.len()..].trim_start();
    if !ident.is_empty() && after_ident.starts_with('=') && !after_ident.starts_with("==") {
        if !matches!(ident, "key" | "prefix" | "name" | "table") {
            return None;
        }
        rest = after_ident[1..].trim_start();
    }
    // A raw or unicode prefix leaves the literal's text unchanged.
    if let Some(unprefixed) = rest.strip_prefix(['r', 'R', 'u', 'U'])
        && unprefixed.starts_with(['"', '\''])
    {
        rest = unprefixed;
    }
    let quote = rest.chars().next().filter(|c| matches!(c, '"' | '\'' | '`'))?;
    let body = &rest[1..];
    if body.starts_with(quote) && body[1..].starts_with(quote) {
        // A triple-quoted string.
        return None;
    }
    let close = body.find(quote)?;
    let literal = &body[..close];
    if literal.contains('\\') || literal.contains('\n') || (quote == '`' && literal.contains("${")) {
        return None;
    }
    let tail = body[close + quote.len_utf8()..].trim_start();
    tail.starts_with([',', ')']).then(|| literal.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn py(source: &str) -> Reach {
        derive("python", &json!({"source": source}))
    }

    #[test]
    fn literal_state_keys_land_in_their_lists() {
        let r = py("import mcphost\nmcphost.state.set(\"orders/1\", 1)\nx = mcphost.state.get('cfg/a')\n");
        assert_eq!(r.state.writes, BTreeSet::from(["orders/1".to_string()]));
        assert_eq!(r.state.reads, BTreeSet::from(["cfg/a".to_string()]));
        assert!(r.modules.contains("state"));
    }

    #[test]
    fn non_literal_key_is_dynamic_in_that_list_only() {
        let r = py("mcphost.state.set(key, 1)\n");
        assert_eq!(r.state.writes, BTreeSet::from([DYNAMIC.to_string()]));
        assert!(r.state.reads.is_empty() && r.state.tables.is_empty());
    }

    #[test]
    fn alias_is_dynamic_everywhere() {
        let r = py("s = mcphost.state\ns.set('a', 1)\n");
        for set in [&r.state.reads, &r.state.writes, &r.state.tables] {
            assert!(set.contains(DYNAMIC));
        }
    }

    #[test]
    fn concatenated_key_is_dynamic() {
        assert!(py("mcphost.state.get('a' + x)\n").state.reads.contains(DYNAMIC));
    }

    #[test]
    fn network_makes_hosts_dynamic() {
        let r = derive("js", &json!({"source": "", "network": "public"}));
        assert!(r.dynamic_hosts && r.network == "public");
    }
}
