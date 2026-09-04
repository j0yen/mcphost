//! Deterministic, offline `args_schema` and `requirements` inference for the
//! `python` and `http` kinds. PRD: `PRD-mcphost-tool-infer.md`.
//!
//! ## Scoped decisions (documented here rather than silently assumed)
//!
//! * **In-process static analysis, not a second sandboxed subprocess.** The
//!   PRD's Technical Considerations suggest reusing the `python` kind's
//!   existing in-sandbox AST check (`python.rs::ast_check`) so untrusted
//!   source is never parsed outside that boundary. That check runs from
//!   [`super::Kind::validate_async`], which is `async`; but
//!   [`super::Kind::describe`] -- which must return the *same* inferred
//!   schema on every `tools/list` -- is synchronous and unchangeable (the
//!   PRD's own non-goal: "No change to the Kind trait"). Round-tripping a
//!   subprocess on every `describe()` call is both infeasible from a sync
//!   function and far outside the 100ms publish-latency budget applied
//!   repeatedly. Instead, inference here is a pure, in-process text scan:
//!   it never calls `eval`/`exec`/`import`, only reads characters -- so it
//!   does not reopen the "untrusted code execution in the host process"
//!   risk the sandbox exists to prevent (this is exactly what AC17 checks:
//!   no network, no tenant code executed, and a text scan trivially
//!   satisfies that everywhere, in-process or sandboxed). The sandboxed
//!   `ast_check` still runs first and still owns syntax validation (AC16);
//!   this module is reached only after that succeeds.
//! * **`validate_async` gates, `describe`/`call` recompute.** Because the
//!   control plane and the stored `spec` are unchanged (non-goal), the
//!   derived schema is never persisted -- `python::validate_async` calls
//!   into this module purely to fail publish early (AC7/AC10/AC11) with a
//!   structured error; `describe`/`call` call the same pure functions again
//!   on the same `source`, which is deterministic by construction (AC13),
//!   so the two calls always agree.
//! * **No type narrowing without evidence.** Per the PRD's non-goal, a
//!   property with no resolvable default is emitted as `{}` (any JSON type
//!   accepted) rather than guessed as `"string"` -- narrowing would reject
//!   a valid call the tenant's own code never asked to reject.
//! * **`http` inference runs synchronously inside `validate`.** Unlike
//!   `python`, template parsing (`minijinja`) has no untrusted-execution
//!   concern -- it is a pure parse, not an interpreter -- so there is no
//!   sandbox boundary to respect and no need to defer to an async step.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use serde_json::{Map, Value, json};

use super::KindError;

// ---- static data (requirement 17: data, not code) --------------------------

static STDLIB_JSON: &str = include_str!("../../infer-data/python-stdlib.json");
static IMPORT_MAP_JSON: &str = include_str!("../../infer-data/python-import-map.json");

fn stdlib_modules() -> &'static BTreeSet<String> {
    static CELL: OnceLock<BTreeSet<String>> = OnceLock::new();
    CELL.get_or_init(|| {
        serde_json::from_str::<Vec<String>>(STDLIB_JSON)
            .expect("infer-data/python-stdlib.json must be a JSON array of strings")
            .into_iter()
            .collect()
    })
}

fn import_map() -> &'static BTreeMap<String, String> {
    static CELL: OnceLock<BTreeMap<String, String>> = OnceLock::new();
    CELL.get_or_init(|| {
        serde_json::from_str::<BTreeMap<String, String>>(IMPORT_MAP_JSON)
            .expect("infer-data/python-import-map.json must be a JSON object of strings")
    })
}

// ---- masking: strip comments, index string literals -----------------------
//
// A hand-rolled scanner rather than a full tokenizer: it only needs to (a)
// know which byte ranges are comments (dropped) or string-literal bodies
// (kept out of the "code" view but indexed by their content, so a literal
// dict key or default value can still be read back), and (b) never
// misinterpret text inside a string or comment as an `args[...]` access.

struct StrLit {
    /// Byte offset of the opening quote (or the first quote of `'''`/`"""`).
    start: usize,
    /// Byte offset one past the closing quote(s).
    end: usize,
    content: String,
    /// `f"..."` / `f'''...'''` -- not a static literal, never used as a key
    /// or a default.
    is_fstring: bool,
}

struct Masked {
    /// Same length and byte offsets as the original source; comments and
    /// string-literal bodies are replaced with spaces so pattern matching
    /// below can use plain substring search without tripping on either.
    code: Vec<u8>,
    lits: Vec<StrLit>,
}

fn mask(source: &str) -> Masked {
    let bytes = source.as_bytes();
    let n = bytes.len();
    let mut code = bytes.to_vec();
    let mut lits = Vec::new();
    let mut i = 0;
    while i < n {
        let c = bytes[i];
        if c == b'#' {
            while i < n && bytes[i] != b'\n' {
                code[i] = b' ';
                i += 1;
            }
            continue;
        }
        if c == b'"' || c == b'\'' {
            let quote = c;
            // Up to two immediately-preceding ASCII letters are a string
            // prefix (f/r/b/u in any case/order Python allows).
            let mut prefix_start = i;
            while prefix_start > 0
                && (i - prefix_start) < 2
                && bytes[prefix_start - 1].is_ascii_alphabetic()
            {
                prefix_start -= 1;
            }
            let is_fstring = source[prefix_start..i]
                .bytes()
                .any(|b| b == b'f' || b == b'F');
            let triple = i + 2 < n && bytes[i + 1] == quote && bytes[i + 2] == quote;
            let content_start = if triple { i + 3 } else { i + 1 };
            let mut j = content_start;
            let mut closed_at = None;
            while j < n {
                if bytes[j] == b'\\' && j + 1 < n {
                    j += 2;
                    continue;
                }
                if triple {
                    if j + 2 < n && bytes[j] == quote && bytes[j + 1] == quote && bytes[j + 2] == quote
                    {
                        closed_at = Some(j);
                        break;
                    }
                } else if bytes[j] == quote || bytes[j] == b'\n' {
                    if bytes[j] == quote {
                        closed_at = Some(j);
                    }
                    break;
                }
                j += 1;
            }
            let content_end = j.min(n);
            let content = source[content_start..content_end].to_string();
            let end = match closed_at {
                Some(pos) => pos + if triple { 3 } else { 1 },
                None => content_end,
            };
            for byte in code.iter_mut().take(content_end).skip(content_start) {
                *byte = b' ';
            }
            lits.push(StrLit {
                start: i,
                end,
                content,
                is_fstring,
            });
            i = end;
            continue;
        }
        i += 1;
    }
    Masked { code, lits }
}

impl Masked {
    fn code_str(&self) -> &str {
        // `code` is byte-for-byte the same length as the (valid UTF-8)
        // source with only ASCII bytes (quotes/comment markers/whitespace)
        // ever overwritten, so it stays valid UTF-8.
        std::str::from_utf8(&self.code).unwrap_or("")
    }

    fn lit_at(&self, pos: usize) -> Option<&StrLit> {
        self.lits.iter().find(|l| l.start == pos)
    }
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// True if `haystack[at..]` starts with `needle` and `at` is a word
/// boundary (not preceded by an identifier byte).
fn word_match(haystack: &str, at: usize, needle: &str) -> bool {
    let bytes = haystack.as_bytes();
    if at > 0 && is_ident_byte(bytes[at - 1]) {
        return false;
    }
    haystack[at..].starts_with(needle)
}

/// Every byte offset in `code` where `needle` occurs at a word boundary.
fn find_word_boundary_occurrences(code: &str, needle: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut start = 0;
    while let Some(rel) = code[start..].find(needle) {
        let at = start + rel;
        if word_match(code, at, needle) {
            out.push(at);
        }
        start = at + 1;
        if start >= code.len() {
            break;
        }
    }
    out
}

fn skip_ws(code: &str, mut i: usize) -> usize {
    let bytes = code.as_bytes();
    while i < bytes.len() && (bytes[i] as char).is_whitespace() {
        i += 1;
    }
    i
}

// ---- python: args_schema inference (requirements 2/3/7/15) ----------------

#[derive(Debug, Clone)]
struct PyKey {
    name: String,
    required: bool,
    default: Option<Value>,
}

/// A literal JSON scalar spelled out in Python syntax immediately after a
/// `,` in `args.get("k", <default>)`: a string (already captured as a
/// [`StrLit`] elsewhere), `True`/`False`/`None`, or a decimal integer/float.
fn parse_py_scalar(text: &str) -> Option<Value> {
    let t = text.trim();
    match t {
        "True" => Some(Value::Bool(true)),
        "False" => Some(Value::Bool(false)),
        "None" => Some(Value::Null),
        _ => {
            if let Ok(i) = t.parse::<i64>() {
                Some(json!(i))
            } else {
                t.parse::<f64>().ok().map(|f| json!(f))
            }
        }
    }
}

fn property_for_default(default: &Value) -> Value {
    let ty = match default {
        Value::String(_) => Some("string"),
        Value::Bool(_) => Some("boolean"),
        Value::Number(n) if n.is_i64() || n.is_u64() => Some("integer"),
        Value::Number(_) => Some("number"),
        Value::Null => None,
        _ => None,
    };
    match ty {
        Some(t) => json!({"type": t, "default": default}),
        None => json!({"default": default}),
    }
}

/// Scans `source` for `args["<k>"]` (required) and `args.get("<k>")` /
/// `args.get("<k>", <default>)` (optional) accesses. Returns `Err` only
/// when the source clearly attempts to read `args` by subscript/`.get()`
/// but not one such access resolved to a literal key (requirement 7,
/// AC11) -- a source that never touches `args` at all yields a permissive
/// empty-object schema, not an error.
fn collect_py_keys(source: &str) -> Result<Vec<PyKey>, KindError> {
    let masked = mask(source);
    let code = masked.code_str();
    let mut keys: BTreeMap<String, PyKey> = BTreeMap::new();
    let mut any_access_attempt = false;

    for at in find_word_boundary_occurrences(code, "args[") {
        any_access_attempt = true;
        let after = at + "args[".len();
        let q = skip_ws(code, after);
        let Some(lit) = masked.lit_at(q) else { continue };
        if lit.is_fstring {
            continue;
        }
        let close = skip_ws(code, lit.end);
        if code.as_bytes().get(close) != Some(&b']') {
            continue;
        }
        keys.entry(lit.content.clone()).or_insert(PyKey {
            name: lit.content.clone(),
            required: true,
            default: None,
        });
    }

    for at in find_word_boundary_occurrences(code, "args.get(") {
        any_access_attempt = true;
        let after = at + "args.get(".len();
        let q = skip_ws(code, after);
        let Some(lit) = masked.lit_at(q) else { continue };
        if lit.is_fstring {
            continue;
        }
        let mut default: Option<Value> = None;
        let next = skip_ws(code, lit.end);
        if code.as_bytes().get(next) == Some(&b',') {
            let default_start = skip_ws(code, next + 1);
            if let Some(dlit) = masked.lit_at(default_start) {
                if !dlit.is_fstring {
                    default = Some(Value::String(dlit.content.clone()));
                }
            } else if let Some(close_rel) = code[default_start..].find([')', ',']) {
                let literal_text = &code[default_start..default_start + close_rel];
                default = parse_py_scalar(literal_text);
            }
        }
        let entry = keys.entry(lit.content.clone()).or_insert(PyKey {
            name: lit.content.clone(),
            required: false,
            default: None,
        });
        // A key seen as both `args["k"]` and `args.get("k")` stays required
        // (AC3 only exercises the reverse combination, but requiredness
        // should never be loosened by a later, more permissive access).
        if default.is_some() {
            entry.default = default;
        }
    }

    if keys.is_empty() && any_access_attempt {
        return Err(KindError::structured(
            "args_schema_not_inferable",
            "source reads 'args' but no literal key could be inferred from it; supply args_schema explicitly",
        ));
    }

    Ok(keys.into_values().collect())
}

/// Requirements 2/3/15: builds the `args_schema` object from the source's
/// `args[...]` / `args.get(...)` accesses.
pub fn infer_python_args_schema(source: &str) -> Result<Value, KindError> {
    let keys = collect_py_keys(source)?;
    let mut properties = Map::new();
    let mut required: Vec<String> = Vec::new();
    for key in &keys {
        let prop = match &key.default {
            Some(default) => property_for_default(default),
            None => json!({}),
        };
        properties.insert(key.name.clone(), prop);
        if key.required {
            required.push(key.name.clone());
        }
    }
    required.sort();
    let mut schema = json!({
        "type": "object",
        "properties": Value::Object(properties),
    });
    if !required.is_empty() {
        schema["required"] = Value::Array(required.into_iter().map(Value::String).collect());
    }
    // Requirement 3: the inferred schema must itself pass the same check an
    // authored schema passes.
    jsonschema::validator_for(&schema)
        .map_err(|e| KindError::Exec(format!("inferred args_schema failed self-check: {e}")))?;
    Ok(schema)
}

// ---- python: requirements inference (requirements 5/6) --------------------

/// Top-level (column-0, outside any string literal) `import x[.y][ as z]` /
/// `from x[.y] import ...` module names. Relative imports (`from . import
/// x`) have no PyPI package and are skipped.
fn top_level_imports(source: &str) -> BTreeSet<String> {
    let masked = mask(source);
    let mut out = BTreeSet::new();
    let mut offset = 0usize;
    for line in masked.code_str().split_inclusive('\n') {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        let line_start = offset;
        offset += line.len();
        if indent != 0 {
            continue;
        }
        // Skip a line whose start falls inside a (masked) string literal,
        // e.g. a module docstring line that happens to read "import foo".
        if masked.lits.iter().any(|l| l.start <= line_start && line_start < l.end) {
            continue;
        }
        let stmt = trimmed.trim_end();
        let rest = if let Some(r) = stmt.strip_prefix("import ") {
            Some(r)
        } else {
            stmt.strip_prefix("from ").map(|r| r.split(" import").next().unwrap_or(r))
        };
        let Some(rest) = rest else { continue };
        for module in rest.split(',') {
            let module = module.trim();
            let module = module.split(" as ").next().unwrap_or(module).trim();
            if module.is_empty() || module.starts_with('.') {
                continue;
            }
            let top = module.split('.').next().unwrap_or(module);
            if !top.is_empty() {
                out.insert(top.to_string());
            }
        }
    }
    out
}

/// Requirements 5/6: modules already covered by the standard library are
/// ignored; a module in [`import_map`] is resolved to its distribution
/// name; anything else fails publish naming the module (AC10) rather than
/// being guessed.
pub fn infer_python_requirements(source: &str) -> Result<Vec<String>, KindError> {
    let mut out = BTreeSet::new();
    for module in top_level_imports(source) {
        if stdlib_modules().contains(&module) {
            continue;
        }
        match import_map().get(&module) {
            Some(dist) => {
                out.insert(dist.clone());
            }
            None => {
                return Err(KindError::structured_with(
                    "requirement_not_inferable",
                    format!(
                        "cannot infer a PyPI requirement for import '{module}'; supply requirements explicitly"
                    ),
                    json!({"module": module}),
                ));
            }
        }
    }
    Ok(out.into_iter().collect())
}

// ---- http: args_schema inference (requirement 4) ---------------------------

/// Every undeclared root variable minijinja's own parser finds in
/// `template`, per the PRD's Technical Considerations ("placeholder
/// extraction should come from the template engine's own parse ... so
/// inference and rendering can never disagree"). `secret` is this crate's
/// reserved template namespace (`{{ secret.name }}`), never a real
/// argument, so it's filtered here rather than by the caller.
fn template_variables(field: &str, template: &str) -> Result<BTreeSet<String>, KindError> {
    let mut env = minijinja::Environment::empty();
    env.add_template_owned(field.to_string(), template.to_string())
        .map_err(|e| {
            KindError::structured_with(
                "template_error",
                format!("{field}: {e}"),
                json!({"field": field}),
            )
        })?;
    let tmpl = env.get_template(field).map_err(|e| {
        KindError::structured_with(
            "template_error",
            format!("{field}: {e}"),
            json!({"field": field}),
        )
    })?;
    Ok(tmpl
        .undeclared_variables(false)
        .into_iter()
        .filter(|v| v != "secret")
        .collect())
}

/// Requirement 4: derives `args_schema` from every placeholder referenced
/// across `url`/`headers`/`query`/`body`, excluding `secret.*` references.
/// `fields` is `(dotted-path, template-text)` -- the same shape `http.rs`'s
/// own `template_fields` already produces, reused by the caller so this
/// module doesn't need to know about `HttpSpec`'s shape.
pub fn infer_http_args_schema(fields: &[(String, String)]) -> Result<Value, KindError> {
    let mut names: BTreeSet<String> = BTreeSet::new();
    for (field, template) in fields {
        names.extend(template_variables(field, template)?);
    }
    let mut properties = Map::new();
    for name in &names {
        properties.insert(name.clone(), json!({}));
    }
    let required: Vec<Value> = names.iter().cloned().map(Value::String).collect();
    let mut schema = json!({
        "type": "object",
        "properties": Value::Object(properties),
    });
    if !required.is_empty() {
        schema["required"] = Value::Array(required);
    }
    jsonschema::validator_for(&schema)
        .map_err(|e| KindError::Exec(format!("inferred args_schema failed self-check: {e}")))?;
    Ok(schema)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_key_from_subscript() {
        let schema = infer_python_args_schema("def main(args):\n    return args[\"city\"]\n").unwrap(); // allowlist: test-only unwrap on a fixed literal
        assert_eq!(schema["required"], json!(["city"]));
        assert!(schema["properties"]["city"].is_object());
    }

    #[test]
    fn optional_key_from_get() {
        let schema =
            infer_python_args_schema("def main(args):\n    return args.get(\"units\")\n").unwrap(); // allowlist: test-only unwrap on a fixed literal
        assert_eq!(schema["properties"].as_object().unwrap().len(), 1);
        assert!(
            schema.get("required").is_none()
                || !schema["required"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("units"))
        );
    }

    #[test]
    fn default_value_captured() {
        let schema = infer_python_args_schema(
            "def main(args):\n    return args.get(\"units\", \"metric\")\n",
        )
        .unwrap();
        assert_eq!(schema["properties"]["units"]["default"], json!("metric"));
        assert_eq!(schema["properties"]["units"]["type"], json!("string"));
    }

    #[test]
    fn no_args_access_is_not_an_error() {
        let schema = infer_python_args_schema("def main(args):\n    return {\"ok\": True}\n").unwrap(); // allowlist: test-only unwrap on a fixed literal
        assert_eq!(schema["properties"], json!({}));
    }

    #[test]
    fn unresolvable_access_is_an_error() {
        let err = infer_python_args_schema(
            "def main(args):\n    k = 'city'\n    return args[k]\n",
        )
        .unwrap_err();
        assert!(matches!(err, KindError::Structured { code: "args_schema_not_inferable", .. }));
    }

    #[test]
    fn comment_and_string_lookalikes_ignored() {
        let schema = infer_python_args_schema(
            "def main(args):\n    # args[\"decoy\"]\n    s = \"args['also_decoy']\"\n    return args[\"real\"]\n",
        )
        .unwrap();
        assert_eq!(schema["required"], json!(["real"]));
        assert_eq!(schema["properties"].as_object().unwrap().len(), 1);
    }

    #[test]
    fn stdlib_only_needs_no_requirements() {
        let reqs = infer_python_requirements("import json\nimport os\n\ndef main(args):\n    return {}\n").unwrap(); // allowlist: test-only unwrap on a fixed literal
        assert!(reqs.is_empty());
    }

    #[test]
    fn mapped_import_resolves() {
        let reqs = infer_python_requirements("import requests\n\ndef main(args):\n    return {}\n").unwrap(); // allowlist: test-only unwrap on a fixed literal
        assert_eq!(reqs, vec!["requests".to_string()]);
    }

    #[test]
    fn unmapped_import_errors_naming_the_module() {
        let err = infer_python_requirements("import definitely_not_a_known_package\n\ndef main(args):\n    return {}\n").unwrap_err();
        match err {
            KindError::Structured { code, data, .. } => {
                assert_eq!(code, "requirement_not_inferable");
                assert_eq!(data["module"], json!("definitely_not_a_known_package"));
            }
            other => panic!("expected Structured, got {other:?}"),
        }
    }

    #[test]
    fn indented_import_is_not_top_level() {
        let reqs = infer_python_requirements(
            "def main(args):\n    import definitely_not_a_known_package\n    return {}\n",
        )
        .unwrap();
        assert!(reqs.is_empty());
    }

    #[test]
    fn deterministic_property_order() {
        let a = infer_python_args_schema(
            "def main(args):\n    return args[\"zeta\"], args[\"alpha\"], args.get(\"middle\")\n",
        )
        .unwrap();
        let b = infer_python_args_schema(
            "def main(args):\n    return args[\"zeta\"], args[\"alpha\"], args.get(\"middle\")\n",
        )
        .unwrap();
        assert_eq!(serde_json::to_vec(&a).unwrap(), serde_json::to_vec(&b).unwrap());
    }

    #[test]
    fn http_schema_excludes_secret_placeholders() {
        let fields = vec![
            ("url".to_string(), "https://api.example.com/{{ city }}".to_string()),
            ("headers.Authorization".to_string(), "Bearer {{ secret.api_key }}".to_string()),
        ];
        let schema = infer_http_args_schema(&fields).unwrap(); // allowlist: test-only unwrap on a fixed literal
        assert_eq!(schema["required"], json!(["city"]));
        assert!(schema["properties"].get("secret").is_none());
    }

    #[test]
    fn http_schema_covers_all_template_fields() {
        let fields = vec![
            ("url".to_string(), "https://api.example.com/{{ city }}".to_string()),
            ("headers.X-Region".to_string(), "{{ region }}".to_string()),
            ("query.units".to_string(), "{{ units }}".to_string()),
            ("body.name".to_string(), "{{ customer_name }}".to_string()),
        ];
        let schema = infer_http_args_schema(&fields).unwrap(); // allowlist: test-only unwrap on a fixed literal
        let required: BTreeSet<String> = schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            required,
            BTreeSet::from([
                "city".to_string(),
                "region".to_string(),
                "units".to_string(),
                "customer_name".to_string(),
            ])
        );
    }
}
