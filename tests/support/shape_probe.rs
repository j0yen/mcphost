//! Shared helpers for the `shp_ac*` proofs (PRD-mcphost-schema-shape-agreement):
//! `SHAPE_PROBE_STRINGS`, the one corpus of schema-valid strings a hand-written
//! shape check is likely to refuse, and the generator of shape probes over a
//! descriptor (one string leaf or one array at a time, the rest of the call at
//! its `vbd_calls` placeholder).
//!
//! Every chain `schema_mismatch` value a future run reports is added to
//! [`SHAPE_PROBE_STRINGS`], with a test that reds before the fix.
//!
//! `allow(dead_code)`: each including test file uses only part of this.
#![allow(dead_code)]

use crate::vbd_calls::{call_for, placeholder};
use serde_json::{Value, json};

/// The conformance probe value, a dotted value, a value with a space, a
/// non-ASCII value, and a value with `:` (the `content_base64` refusal at offset 5).
pub const SHAPE_PROBE_STRINGS: &[&str] = &["toolconf-probe", "toolconf.probe", "tool conf", "caf\u{e9}", "tool:conf"];

/// One step of a path into a call: an object key or the first array item.
#[derive(Clone, Debug)]
pub enum Seg {
    Key(String),
    First,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SiteKind {
    /// A string leaf lacking `pattern`/`enum`/`const`.
    String,
    /// An array lacking `uniqueItems` whose `maxItems` allows two.
    Dup { min: usize, items: Value },
}

#[derive(Clone, Debug)]
pub struct Site {
    pub path: Vec<Seg>,
    pub kind: SiteKind,
    /// The schema of each container on the way down (for placeholder filling).
    pub parents: Vec<Value>,
}

impl Site {
    /// RFC 6901 pointer of the leaf; an array's first item is `/0`.
    pub fn pointer(&self) -> String {
        self.path
            .iter()
            .map(|s| match s {
                Seg::Key(k) => format!("/{k}"),
                Seg::First => "/0".to_string(),
            })
            .collect()
    }

    /// The property name a refusal would name: the last key on the path.
    pub fn property(&self) -> String {
        self.path
            .iter()
            .rev()
            .find_map(|s| if let Seg::Key(k) = s { Some(k.clone()) } else { None })
            .unwrap_or_default()
    }
}

fn has_any(schema: &Value, keys: &[&str]) -> bool {
    keys.iter().any(|k| schema.get(*k).is_some())
}

fn is_type(schema: &Value, ty: &str) -> bool {
    match schema.get("type") {
        Some(Value::String(t)) => t == ty,
        Some(Value::Array(ts)) => ts.iter().any(|t| t == ty),
        _ => false,
    }
}

/// Every shape-probe site of one input schema: string leaves without
/// `pattern`/`enum`/`const`, and arrays without `uniqueItems` that allow two items.
pub fn sites(schema: &Value) -> Vec<Site> {
    let mut out = Vec::new();
    walk(schema, &mut Vec::new(), &mut Vec::new(), &mut out, 0);
    out
}

fn walk(schema: &Value, path: &mut Vec<Seg>, parents: &mut Vec<Value>, out: &mut Vec<Site>, depth: usize) {
    if depth > 6 {
        return;
    }
    if is_type(schema, "string") && !has_any(schema, &["pattern", "enum", "const"]) && !path.is_empty() {
        out.push(Site { path: path.clone(), kind: SiteKind::String, parents: parents.clone() });
    }
    if is_type(schema, "array") && !path.is_empty() {
        let max_ok = schema.get("maxItems").and_then(Value::as_u64).is_none_or(|m| m >= 2);
        let unique = schema.get("uniqueItems").and_then(Value::as_bool).unwrap_or(false);
        let min = schema.get("minItems").and_then(Value::as_u64).unwrap_or(0) as usize;
        let max = schema.get("maxItems").and_then(Value::as_u64).map_or(usize::MAX, |m| m as usize);
        if max_ok && !unique && min <= max && min <= 64 {
            let items = schema.get("items").map(placeholder).unwrap_or_else(|| json!("x"));
            out.push(Site { path: path.clone(), kind: SiteKind::Dup { min, items }, parents: parents.clone() });
        }
    }
    if let Some(props) = schema.get("properties").and_then(Value::as_object) {
        for (name, sub) in props {
            if name == "tenant_key" {
                continue;
            }
            path.push(Seg::Key(name.clone()));
            parents.push(schema.clone());
            walk(sub, path, parents, out, depth + 1);
            parents.pop();
            path.pop();
        }
    }
    if let Some(items) = schema.get("items").filter(|i| i.is_object()) {
        path.push(Seg::First);
        parents.push(schema.clone());
        walk(items, path, parents, out, depth + 1);
        parents.pop();
        path.pop();
    }
    for kw in ["anyOf", "oneOf"] {
        for branch in schema.get(kw).and_then(Value::as_array).into_iter().flatten() {
            // A branch repeats the parent's own properties as `required`-only
            // fragments; walk it for any property it types itself.
            walk(branch, path, parents, out, depth + 1);
        }
    }
}

/// The placeholder call for `schema` with `site` set to `value`, creating the
/// containers on the way from each parent's own placeholder.
pub fn call_with(schema: &serde_json::Map<String, Value>, site: &Site, value: Value) -> Value {
    let mut call = call_for(schema);
    set_at(&mut call, &site.path, &site.parents, value);
    call
}

fn set_at(node: &mut Value, path: &[Seg], parents: &[Value], value: Value) {
    let Some((head, rest)) = path.split_first() else {
        *node = value;
        return;
    };
    match head {
        Seg::Key(k) => {
            if !node.is_object() {
                *node = json!({});
            }
            let sub_schema = parents.first().and_then(|p| p.pointer(&format!("/properties/{k}"))).cloned();
            let obj = node.as_object_mut().expect("object");
            let child = obj.entry(k.clone()).or_insert_with(|| sub_schema.as_ref().map(placeholder).unwrap_or(Value::Null));
            set_at(child, rest, &parents[1.min(parents.len())..], value);
        }
        Seg::First => {
            if !node.is_array() || node.as_array().is_some_and(Vec::is_empty) {
                let item = parents.first().and_then(|p| p.get("items")).map(placeholder).unwrap_or(Value::Null);
                *node = json!([item]);
            }
            let child = &mut node.as_array_mut().expect("array")[0];
            set_at(child, rest, &parents[1.min(parents.len())..], value);
        }
    }
}
