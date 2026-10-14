//! Publish-time lint for python tool sources (PRD-mcphost-publish-time-tool-lint).
//!
//! Three textual rules catch failure shapes the 10-08 explore sessions only
//! met at call time: `mcphost.` used without `import mcphost`, a
//! string-typed argument subscripted with a string key, and a `main` return
//! that is not JSON-serializable. Each rule is one [`LintRule`] row in
//! [`LINT_RULES`]; findings are advisory and never block a publish.
//!
//! Detectors read the source through [`super::infer`]'s `mask` (comments and
//! string bodies blanked, same byte offsets) and the same `args_schema` the
//! validator enforces (declared, else inferred), so there is no second
//! parser. No detector may panic: [`lint_source`] runs each under
//! `catch_unwind` and a panicking detector yields no findings.

use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};

use serde_json::{Value, json};

/// Longest `source_line` a finding carries.
const SOURCE_LINE_MAX: usize = 200;

/// One detector hit: the 1-based `line` plus the values its rule's
/// `message`/`fix` templates substitute (`{line}` is always available).
pub struct Finding {
    pub line: usize,
    pub vars: Vec<(&'static str, String)>,
}

/// Everything a detector reads: the masked source and each argument's
/// schema type (`args_schema` declared on the spec, else inferred).
pub struct LintCtx<'a> {
    pub source: &'a str,
    pub masked: &'a str,
    pub arg_types: BTreeMap<String, String>,
}

impl LintCtx<'_> {
    /// `(1-based line, original line, masked line)` for every source line.
    fn lines(&self) -> Vec<(usize, &str, &str)> {
        self.source
            .lines()
            .zip(self.masked.lines())
            .enumerate()
            .map(|(i, (o, m))| (i + 1, o, m))
            .collect()
    }
}

/// One row of the lint table. `message`/`fix` are templates with `{name}`
/// placeholders filled from [`Finding::vars`]; `test_name` is the
/// integration test that proves the rule; `fixture` is a source the rule
/// fires on (the table test renders its templates against it).
pub struct LintRule {
    pub id: &'static str,
    pub detect: fn(&LintCtx) -> Vec<Finding>,
    pub message: &'static str,
    pub fix: &'static str,
    pub test_name: &'static str,
    pub fixture: &'static str,
}

pub const LINT_RULES: &[LintRule] = &[
    LintRule {
    id: "mcphost_not_imported",
    detect: detect_mcphost_not_imported,
    message: "line {line} uses `mcphost.` but the source never imports mcphost; the call fails with NameError",
    fix: "add `import mcphost` at line 1",
    test_name: "missing_import_reports_first_use_line_and_publishes",
    fixture: "def main(args):\n    mcphost.progress(1, \"x\")\n    return {}\n",
},
    LintRule {
        id: "string_arg_subscripted",
        detect: detect_string_arg_subscripted,
        message: "line {line} indexes {expr} with a string key but `{key}` is typed string in args_schema; parse it with json.loads(args[\"{key}\"]) or declare `{key}` as type object",
        fix: "use json.loads(args[\"{key}\"]) before indexing, or declare `{key}` as type object in args_schema",
        test_name: "string_typed_argument_subscripted_reports_line_and_fix",
        fixture: "import json\n\n\ndef main(args):\n    payload = args.get(\"payload\", \"\")\n    return {\"id\": payload[\"id\"]}\n",
    },
    LintRule {
        id: "return_not_json",
        detect: detect_return_not_json,
        message: "line {line} returns `{expr}` ({what}), which is not JSON-serializable; {fix}",
        fix: "{fix}",
        test_name: "non_json_return_reports_return_line_and_per_kind_fix",
        fixture: "import requests\n\n\ndef main(args):\n    resp = requests.get(args[\"url\"])\n    return resp\n",
    },
];

/// Replaces every `{name}` in `template` with its value (`{line}` included).
pub fn render(template: &str, line: usize, vars: &[(&'static str, String)]) -> String {
    let mut out = template.replace("{line}", &line.to_string());
    for (name, value) in vars {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}

/// Argument name -> JSON-schema `type` for every typed property of the
/// spec's declared `args_schema`, else of the schema inferred from `source`.
fn arg_types(source: &str, declared: Option<&Value>) -> BTreeMap<String, String> {
    let inferred;
    let schema = match declared {
        Some(s) => s,
        None => {
            inferred = super::infer::infer_python_args_schema_unchecked(source).unwrap_or(Value::Null);
            &inferred
        }
    };
    let mut out = BTreeMap::new();
    if let Some(props) = schema.get("properties").and_then(Value::as_object) {
        for (name, prop) in props {
            if let Some(ty) = prop.get("type").and_then(Value::as_str) {
                out.insert(name.clone(), ty.to_string());
            }
        }
    }
    out
}

/// Runs every rule over `source`; findings sorted by line then rule order.
pub fn lint_source(source: &str, args_schema: Option<&Value>) -> Vec<Value> {
    lint_checked(source, args_schema).0
}

/// [`lint_source`] plus the ids of any rules whose detector panicked (their
/// findings are dropped, the others kept) -- what the no-panic test reads.
pub fn lint_checked(source: &str, args_schema: Option<&Value>) -> (Vec<Value>, Vec<&'static str>) {
    let masked = super::infer::masked_source(source);
    let ctx = LintCtx { source, masked: &masked, arg_types: arg_types(source, args_schema) };
    let masked_lines: Vec<&str> = masked.lines().collect();
    let mut out: Vec<(usize, Value)> = Vec::new();
    let mut panicked = Vec::new();
    for rule in LINT_RULES {
        let Ok(found) = catch_unwind(AssertUnwindSafe(|| (rule.detect)(&ctx))) else {
            panicked.push(rule.id);
            continue;
        };
        for f in found {
            let shown = masked_lines.get(f.line.wrapping_sub(1)).map(|l| l.trim()).unwrap_or("");
            let source_line: String = shown.chars().take(SOURCE_LINE_MAX).collect();
            out.push((
                f.line,
                json!({
                    "rule": rule.id,
                    "line": f.line,
                    "source_line": source_line,
                    "message": render(rule.message, f.line, &f.vars),
                    "fix": render(rule.fix, f.line, &f.vars),
                }),
            ));
        }
    }
    out.sort_by_key(|(line, _)| *line);
    (out.into_iter().map(|(_, v)| v).collect(), panicked)
}

/// Lint for a publish/test `spec`: `[]` unless it carries a python `source`.
pub fn lint_spec(spec: &Value) -> Vec<Value> {
    match spec.get("source").and_then(Value::as_str) {
        Some(source) => lint_source(source, spec.get("args_schema").filter(|v| !v.is_null())),
        None => Vec::new(),
    }
}

// ---- helpers --------------------------------------------------------------

fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Byte offsets where `needle` occurs in `hay` not preceded by an
/// identifier byte or `.` (so `x.mcphost.` and `my_args[` do not match).
fn find_bare(hay: &str, needle: &str) -> Vec<usize> {
    let bytes = hay.as_bytes();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(rel) = hay[from..].find(needle) {
        let at = from + rel;
        if at == 0 || !(is_ident(bytes[at - 1]) || bytes[at - 1] == b'.') {
            out.push(at);
        }
        from = at + needle.len();
    }
    out
}

fn skip_ws(s: &str, mut i: usize) -> usize {
    let b = s.as_bytes();
    while i < b.len() && (b[i] == b' ' || b[i] == b'\t') {
        i += 1;
    }
    i
}

fn leading_ident(s: &str) -> &str {
    let end = s.bytes().position(|b| !is_ident(b)).unwrap_or(s.len());
    &s[..end]
}

// ---- rule: mcphost_not_imported -------------------------------------------

/// True for `import mcphost[.x][ as y]` (any comma part) and `from mcphost import`.
fn is_mcphost_import(trimmed: &str) -> bool {
    if let Some(rest) = trimmed.strip_prefix("import ") {
        return rest.split(',').any(|part| leading_ident(part.trim()) == "mcphost");
    }
    trimmed.strip_prefix("from ").is_some_and(|rest| {
        let rest = rest.trim_start();
        leading_ident(rest) == "mcphost" && rest["mcphost".len()..].trim_start().starts_with("import")
    })
}

fn detect_mcphost_not_imported(ctx: &LintCtx) -> Vec<Finding> {
    let mut first_use = None;
    for (line, _, m) in ctx.lines() {
        let t = m.trim_start();
        if is_mcphost_import(t) {
            return Vec::new();
        }
        if first_use.is_none()
            && !t.starts_with("import ")
            && !t.starts_with("from ")
            && !find_bare(m, "mcphost.").is_empty()
        {
            first_use = Some(line);
        }
    }
    first_use.map(|line| Finding { line, vars: Vec::new() }).into_iter().collect()
}

// ---- rule: string_arg_subscripted -----------------------------------------

/// Parses `args["k"]` / `args.get("k"[, default])` starting at `at` (the `a`
/// of `args`) on one line; returns the key (read from the original line, the
/// masked one has its body blanked) and the offset just past the access.
fn args_access(m: &str, o: &str, at: usize) -> Option<(String, usize)> {
    let rest = m.get(at..)?;
    let (open, is_get) = if rest.starts_with("args[") {
        (at + 5, false)
    } else if rest.starts_with("args.get(") {
        (at + 9, true)
    } else {
        return None;
    };
    let p = skip_ws(m, open);
    let q = *m.as_bytes().get(p)?;
    if q != b'"' && q != b'\'' {
        return None;
    }
    let close = p + 1 + m.get(p + 1..)?.find(q as char)?;
    let key = o.get(p + 1..close)?.to_string();
    let after = skip_ws(m, close + 1);
    if !is_get {
        return (m.as_bytes().get(after) == Some(&b']')).then_some((key, after + 1));
    }
    let mut depth = 1usize;
    for (j, b) in m.as_bytes().get(open..)?.iter().enumerate() {
        match b {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some((key, open + j + 1));
                }
            }
            _ => {}
        }
    }
    None
}

/// True when the text at `end` is `[ "..."` (a string-literal subscript).
fn string_subscript_at(m: &str, end: usize) -> bool {
    let b = m.as_bytes();
    let k = skip_ws(m, end);
    if b.get(k) != Some(&b'[') {
        return false;
    }
    matches!(b.get(skip_ws(m, k + 1)), Some(b'"' | b'\''))
}

/// `name = rhs` (not `==`): the target name and the offset of the rhs.
fn assignment(m: &str) -> Option<(&str, usize)> {
    let indent = m.len() - m.trim_start().len();
    let name = leading_ident(&m[indent..]);
    if name.is_empty() || name.as_bytes()[0].is_ascii_digit() {
        return None;
    }
    let eq = skip_ws(m, indent + name.len());
    let b = m.as_bytes();
    if b.get(eq) != Some(&b'=') || b.get(eq + 1) == Some(&b'=') {
        return None;
    }
    Some((name, skip_ws(m, eq + 1)))
}

fn detect_string_arg_subscripted(ctx: &LintCtx) -> Vec<Finding> {
    let is_string = |k: &str| ctx.arg_types.get(k).is_some_and(|t| t == "string");
    let mut bound: BTreeMap<String, String> = BTreeMap::new();
    let mut out = Vec::new();
    for (line, o, m) in ctx.lines() {
        // (key, expression as written)
        let mut hits: Vec<(String, String)> = Vec::new();
        for at in find_bare(m, "args") {
            if let Some((key, end)) = args_access(m, o, at)
                && is_string(&key)
                && string_subscript_at(m, end)
            {
                hits.push((key.clone(), format!("args[\"{key}\"]")));
            }
        }
        for (name, key) in &bound {
            if !is_string(key) {
                continue;
            }
            for at in find_bare(m, name) {
                if string_subscript_at(m, at + name.len()) {
                    hits.push((key.clone(), name.clone()));
                }
            }
        }
        // `for x in args["k"]:` / `for x in name:` walks a string's characters.
        if m.trim_start().starts_with("for ")
            && let Some(i) = m.find(" in ")
        {
            let s = skip_ws(m, i + 4);
            let colon_after = |end: usize| m.as_bytes().get(skip_ws(m, end)) == Some(&b':');
            if let Some((key, end)) = args_access(m, o, s) {
                if is_string(&key) && colon_after(end) {
                    hits.push((key.clone(), format!("args[\"{key}\"]")));
                }
            } else {
                let name = leading_ident(&m[s..]);
                if let Some(key) = bound.get(name)
                    && is_string(key)
                    && colon_after(s + name.len())
                {
                    hits.push((key.clone(), name.to_string()));
                }
            }
        }
        let mut seen: Vec<String> = Vec::new();
        for (key, expr) in hits {
            if !seen.contains(&key) {
                seen.push(key.clone());
                out.push(Finding { line, vars: vec![("key", key), ("expr", expr)] });
            }
        }
        if let Some((name, rhs)) = assignment(m) {
            match args_access(m, o, rhs) {
                Some((key, end)) if m[end..].trim().is_empty() => {
                    bound.insert(name.to_string(), key);
                }
                _ => {
                    bound.remove(name);
                }
            }
        }
    }
    out
}

// ---- rule: return_not_json ------------------------------------------------

#[derive(Clone, Copy)]
enum NonJson {
    Response,
    File,
    Datetime,
    Set,
    Bytes,
    Decimal,
}

impl NonJson {
    fn what(self) -> &'static str {
        match self {
            NonJson::Response => "a requests.Response",
            NonJson::File => "an open file",
            NonJson::Datetime => "a datetime value",
            NonJson::Set => "a set",
            NonJson::Bytes => "bytes",
            NonJson::Decimal => "a Decimal",
        }
    }

    fn fix(self, expr: &str) -> String {
        match self {
            NonJson::Response => format!("return {expr}.json()"),
            NonJson::File => format!("return {expr}.read()"),
            NonJson::Datetime => format!("return {expr}.isoformat()"),
            NonJson::Set => format!("return list({expr})"),
            NonJson::Bytes => format!("return {expr}.decode()"),
            NonJson::Decimal => format!("return float({expr})"),
        }
    }
}

/// True when `m` is one call expression: its first `(` closes at the end.
fn is_single_call(m: &str) -> bool {
    let Some(open) = m.find('(') else { return false };
    let mut depth = 0usize;
    for (i, b) in m.bytes().enumerate().skip(open) {
        match b {
            b'(' => depth += 1,
            b')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return m[i + 1..].trim().is_empty();
                }
            }
            _ => {}
        }
    }
    false
}

/// What non-JSON value the whole expression (`m` masked, `o` original) is.
fn classify_expr(m: &str, o: &str) -> Option<NonJson> {
    const REQUESTS: [&str; 7] = [
        "requests.get(",
        "requests.post(",
        "requests.put(",
        "requests.patch(",
        "requests.delete(",
        "requests.head(",
        "requests.request(",
    ];
    if REQUESTS.iter().any(|p| m.starts_with(p)) {
        return is_single_call(m).then_some(NonJson::Response);
    }
    let kind = if m.starts_with("open(") {
        NonJson::File
    } else if m.starts_with("datetime.") {
        NonJson::Datetime
    } else if m.starts_with("set(") {
        NonJson::Set
    } else if m.starts_with("bytes(") {
        NonJson::Bytes
    } else if m.starts_with("Decimal(") || m.starts_with("decimal.Decimal(") {
        NonJson::Decimal
    } else {
        let (mb, ob) = (m.as_bytes(), o.as_bytes());
        let quoted = mb.len() >= 3
            && matches!(ob[0], b'b' | b'B')
            && matches!(mb[1], b'"' | b'\'')
            && mb[mb.len() - 1] == mb[1]
            && mb[2..mb.len() - 1].iter().all(|&c| c == b' ');
        return quoted.then_some(NonJson::Bytes);
    };
    is_single_call(m).then_some(kind)
}

fn detect_return_not_json(ctx: &LintCtx) -> Vec<Finding> {
    let lines = ctx.lines();
    let is_main = |m: &str| {
        let t = m.trim_start();
        m.len() == t.len() && (t.starts_with("def main(") || t.starts_with("async def main("))
    };
    let Some(start) = lines.iter().position(|(_, _, m)| is_main(m)) else { return Vec::new() };
    let mut bound: BTreeMap<String, NonJson> = BTreeMap::new();
    let mut out = Vec::new();
    for &(line, o, m) in &lines[start + 1..] {
        let t = m.trim_start();
        if t.is_empty() {
            continue;
        }
        if m.len() == t.len() {
            break; // dedent to column 0: main's body is over
        }
        let end = m.trim_end().len();
        if t.starts_with("return ") {
            let off = skip_ws(m, m.len() - t.len() + 7);
            let Some(em) = m.get(off..end) else { continue };
            let eo = o.get(off..end).unwrap_or(em);
            let kind = match bound.get(em) {
                Some(k) if !em.is_empty() && em.bytes().all(is_ident) => Some(*k),
                _ => classify_expr(em, eo),
            };
            if let Some(kind) = kind {
                out.push(Finding {
                    line,
                    vars: vec![
                        ("expr", eo.to_string()),
                        ("what", kind.what().to_string()),
                        ("fix", kind.fix(eo)),
                    ],
                });
            }
        } else if let Some((name, rhs)) = assignment(m) {
            match m.get(rhs..end).zip(o.get(rhs..end)).and_then(|(em, eo)| classify_expr(em, eo)) {
                Some(kind) => {
                    bound.insert(name.to_string(), kind);
                }
                None => {
                    bound.remove(name);
                }
            }
        }
    }
    out
}
