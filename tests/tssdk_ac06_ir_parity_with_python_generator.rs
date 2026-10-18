//! PRD-mcphost-typescript-sdk-generated AC6 (P0) -- Given one fixture tool
//! rendered through both generators, When field names, optionality and enum
//! values are compared, Then they are identical; a divergence names the
//! field. Both generators read schemas through `gen::schema_ir` (R6); this
//! test reads back what each of them WROTE, so a renderer that drifts from
//! the shared reader still fails.

use std::collections::BTreeMap;

use mcphost::r#gen::{schema_ir, sdk_ts};
use mcphost::sdk_python;
use serde_json::json;

/// What both SDKs must agree on about one field.
#[derive(Debug, PartialEq, Clone)]
struct FieldShape {
    optional: bool,
    enum_values: Vec<String>,
}

type Shapes = BTreeMap<String, FieldShape>;

fn quoted(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(i) = rest.find('"') {
        let tail = &rest[i + 1..];
        let Some(j) = tail.find('"') else { break };
        out.push(tail[..j].to_string());
        rest = &tail[j + 1..];
    }
    out
}

fn between<'a>(s: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let i = s.find(open)? + open.len();
    let j = s[i..].find(close)? + i;
    Some(&s[i..j])
}

/// The top-level fields of `class <name>(BaseModel):` in a rendered `models.py`.
fn python_shapes(models: &str, class: &str) -> Shapes {
    let start = models.find(&format!("class {class}(BaseModel):\n")).unwrap_or_else(|| panic!("no class {class}"));
    let mut out = Shapes::new();
    for line in models[start..].lines().skip(1) {
        if !line.is_empty() && !line.starts_with("    ") {
            break;
        }
        let Some(body) = line.strip_prefix("    ") else { continue };
        if body.starts_with("model_config") || body.starts_with(' ') {
            continue;
        }
        let Some((py, rest)) = body.split_once(": ") else { continue };
        let wire = between(rest, "alias=\"", "\"").unwrap_or(py).to_string();
        let optional = rest.contains("default=None");
        let enum_values = between(rest, "Literal[", "]").map(quoted).unwrap_or_default();
        out.insert(wire, FieldShape { optional, enum_values });
    }
    out
}

/// The top-level fields of `export const <name> = z.object({` in a rendered `schemas.ts`.
/// A field whose value is a nested object spans lines; it ends at the next
/// line indented exactly like a top-level field's closing brace (`  })...,`).
fn ts_shapes(schemas: &str, konst: &str) -> Shapes {
    let start = schemas.find(&format!("export const {konst} = z.object({{\n")).unwrap_or_else(|| panic!("no const {konst}"));
    let mut out = Shapes::new();
    let mut pending: Option<(String, Vec<String>)> = None;
    for line in schemas[start..].lines().skip(1) {
        if line.starts_with("})") {
            break;
        }
        let Some(body) = line.strip_prefix("  ") else { continue };
        if let Some((wire, enum_values)) = pending.take() {
            if body.starts_with('}') {
                let optional = body.trim_end_matches(',').ends_with(".optional()");
                out.insert(wire, FieldShape { optional, enum_values });
            } else {
                pending = Some((wire, enum_values));
            }
            continue;
        }
        let Some((key, expr)) = body.split_once(": ") else { continue };
        let wire = key.trim_matches('"').to_string();
        let enum_values = between(expr, "z.enum([", "]").map(quoted).unwrap_or_default();
        if expr.ends_with(',') {
            let optional = expr.trim_end_matches(',').ends_with(".optional()");
            out.insert(wire, FieldShape { optional, enum_values });
        } else {
            pending = Some((wire, enum_values));
        }
    }
    out
}

/// `Ok` when both SDKs agree on every field; the `Err` names the first field that differs.
fn compare(py: &Shapes, ts: &Shapes) -> Result<(), String> {
    for (field, p) in py {
        let Some(t) = ts.get(field) else { return Err(format!("field `{field}` is in the Python SDK only")) };
        if p.optional != t.optional {
            return Err(format!("field `{field}`: optional is {} in Python, {} in TS", p.optional, t.optional));
        }
        if p.enum_values != t.enum_values {
            return Err(format!("field `{field}`: enum {:?} in Python, {:?} in TS", p.enum_values, t.enum_values));
        }
    }
    if let Some(field) = ts.keys().find(|f| !py.contains_key(*f)) {
        return Err(format!("field `{field}` is in the TS SDK only"));
    }
    Ok(())
}

fn fixture_contract() -> Vec<u8> {
    let tool = json!({
        "name": "fixture.thing.make",
        "description": "A fixture tool.",
        "since": "0.1.0",
        "input_schema": {
            "type": "object",
            "additionalProperties": false,
            "required": ["name", "kind", "note"],
            "properties": {
                "name": {"type": "string", "minLength": 1, "maxLength": 40},
                "kind": {"type": "string", "enum": ["echo", "http", "python"]},
                "note": {"type": ["string", "null"]},
                "limit": {"type": "integer", "minimum": 1, "maximum": 50},
                "mode": {"type": ["string", "null"], "enum": ["fast", "slow", null]},
                "class": {"type": "boolean"},
                "tags": {"type": "array", "items": {"type": "string", "enum": ["a", "b"]}},
                "ui": {"type": "object", "properties": {"form": {"type": "boolean"}}}
            }
        },
        "output_schema": {
            "type": "object",
            "required": ["id"],
            "properties": {
                "id": {"type": "string"},
                "state": {"type": "string", "enum": ["new", "done"]},
                "count": {"type": ["integer", "null"]}
            }
        }
    });
    serde_json::to_vec(&json!({"contract_version": 1, "kinds": [], "output_envelope_fields": [], "tools": [tool]})).unwrap()
}

#[test]
fn tssdk_ac06_fixture_tool_has_identical_fields_optionality_and_enums_in_both_sdks() {
    let bytes = fixture_contract();
    let py = sdk_python::render(&bytes).unwrap();
    let ts = sdk_ts::render(&bytes).unwrap();

    for (py_class, ts_const) in [("FixtureThingMakeArgs", "FixtureThingMakeArgs"), ("FixtureThingMakeResult", "FixtureThingMakeResult")] {
        let (p, t) = (python_shapes(&py["models.py"], py_class), ts_shapes(&ts["schemas.ts"], ts_const));
        assert!(p.len() >= 3, "fixture fields were not read back from the Python render: {p:?}");
        compare(&p, &t).unwrap_or_else(|why| panic!("{py_class}: {why}"));
    }

    // and the shared IR says the same thing the renderers wrote
    let contract: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let tool = schema_ir::canonical(&contract).unwrap()[0];
    let ir = schema_ir::object_fields(&tool["input_schema"]);
    let t = ts_shapes(&ts["schemas.ts"], "FixtureThingMakeArgs");
    assert_eq!(ir.iter().map(|f| f.wire).collect::<Vec<_>>(), t.keys().map(String::as_str).collect::<Vec<_>>());
    for f in &ir {
        assert_eq!(t[f.wire].optional, !f.required, "{}: optionality differs from the IR", f.wire);
    }
    assert_eq!(t["kind"].enum_values, ["echo", "http", "python"]);
    assert_eq!(t["mode"].enum_values, ["fast", "slow"]);
    assert!(t["limit"].optional && !t["name"].optional && !t["note"].optional);
    assert!(t.contains_key("class"), "a reserved-word property keeps its wire name");
}

#[test]
fn tssdk_ac06_every_canonical_tool_in_the_contract_agrees_between_the_two_sdks() {
    let bytes = mcphost::call_cli::EMBEDDED_CONTRACT;
    let py = sdk_python::render(bytes).unwrap();
    let ts = sdk_ts::render(bytes).unwrap();
    let names = schema_ir::canonical_names(bytes).unwrap();
    for name in &names {
        let base = sdk_ts::camel(name);
        for suffix in ["Args", "Result"] {
            let class = format!("{base}{suffix}");
            let (p, t) = (python_shapes(&py["models.py"], &class), ts_shapes(&ts["schemas.ts"], &class));
            compare(&p, &t).unwrap_or_else(|why| panic!("{name} {suffix}: {why}"));
        }
    }
    println!("{}/{} tools identical", names.len(), names.len());
}

#[test]
fn tssdk_ac06_a_divergence_names_the_field() {
    let bytes = fixture_contract();
    let py = python_shapes(&sdk_python::render(&bytes).unwrap()["models.py"], "FixtureThingMakeArgs");
    let ts_src = sdk_ts::render(&bytes).unwrap()["schemas.ts"].clone();

    // optionality flipped on `limit`
    let flipped = ts_src.replacen("limit: z.number().int().min(1).max(50).optional(),", "limit: z.number().int().min(1).max(50),", 1);
    assert_ne!(flipped, ts_src, "fixture line not found");
    let why = compare(&py, &ts_shapes(&flipped, "FixtureThingMakeArgs")).unwrap_err();
    assert!(why.contains("`limit`") && why.contains("optional"), "{why}");

    // an enum value dropped from `kind`
    let dropped = ts_src.replacen("z.enum([\"echo\", \"http\", \"python\"])", "z.enum([\"echo\", \"http\"])", 1);
    let why = compare(&py, &ts_shapes(&dropped, "FixtureThingMakeArgs")).unwrap_err();
    assert!(why.contains("`kind`") && why.contains("enum"), "{why}");

    // a field missing on one side
    let missing = ts_src.replacen("  tags: z.array", "  tagz: z.array", 1);
    let why = compare(&py, &ts_shapes(&missing, "FixtureThingMakeArgs")).unwrap_err();
    assert!(why.contains("`tags`") || why.contains("`tagz`"), "{why}");
}
