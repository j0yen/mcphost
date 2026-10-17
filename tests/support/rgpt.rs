//! Runner for the generated `tests/rgpt_<recipe>_golden_path.rs` files
//! (PRD-mcphost-recipe-golden-path-tests R4): boots an in-process
//! `TestServer`, mounts the recipe's fixture routes on a wiremock host,
//! signs up its tenants, then executes the recipe's steps in order and
//! asserts every `expect`. Step data comes from `mcphost::recipes::golden_spec`
//! (embedded in the generated test), so the runner holds no recipe knowledge.

#![allow(dead_code)]

use crate::common::{McpClient, TempDataDir, TestServer, extract_structured, http_kind_registry, python_kind_registry, signup};
use mcphost::recipes::{Expect, check_expect, path_get};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// No single recipe may take longer (PRD AC5).
pub const MAX_RECIPE_WALL: Duration = Duration::from_secs(60);

#[derive(Debug)]
pub struct Trace {
    /// Tool names (or `signed_post`) in the order they ran.
    pub calls: Vec<String>,
    pub elapsed: Duration,
    /// Tool calls actually issued, signups included.
    pub issued: u32,
}

struct Ctx {
    tenants: HashMap<String, (String, String)>,
    fixture_base: String,
    results: Vec<Value>,
}

fn lookup(ctx: &Ctx, inner: &str) -> Value {
    if let Some(rel) = inner.strip_prefix("file:") {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
        return Value::String(std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display())));
    }
    if inner == "fixture_base" {
        return Value::String(ctx.fixture_base.clone());
    }
    if let Some((who, field)) = inner.split_once('.') {
        if let Some((ns, key)) = ctx.tenants.get(who) {
            return match field {
                "ns" => Value::String(ns.clone()),
                "key" => Value::String(key.clone()),
                other => panic!("unknown tenant field `{other}` in ${{{inner}}}"),
            };
        }
        if let Some(n) = who.strip_prefix('s').and_then(|d| d.parse::<usize>().ok()) {
            let result = ctx.results.get(n - 1).unwrap_or_else(|| panic!("${{{inner}}}: step {n} has not run"));
            return path_get(result, &format!("$.{field}"))
                .unwrap_or_else(|| panic!("${{{inner}}}: no `{field}` in {result}"))
                .clone();
        }
    }
    panic!("unresolvable placeholder ${{{inner}}}")
}

fn subst(v: &Value, ctx: &Ctx) -> Value {
    match v {
        Value::String(s) => {
            if let Some(inner) = s.strip_prefix("${").and_then(|r| r.strip_suffix('}'))
                && !inner.contains("${")
            {
                return lookup(ctx, inner);
            }
            let mut out = String::new();
            let mut rest = s.as_str();
            while let Some(start) = rest.find("${") {
                out.push_str(&rest[..start]);
                let end = rest[start..].find('}').expect("unterminated ${") + start;
                match lookup(ctx, &rest[start + 2..end]) {
                    Value::String(t) => out.push_str(&t),
                    other => out.push_str(&other.to_string()),
                }
                rest = &rest[end + 1..];
            }
            out.push_str(rest);
            Value::String(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(|x| subst(x, ctx)).collect()),
        Value::Object(m) => Value::Object(m.iter().map(|(k, x)| (k.clone(), subst(x, ctx))).collect()),
        other => other.clone(),
    }
}

fn hmac_sha256_hex(secret: &str, body: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut key_block = [0u8; 64];
    let key = secret.as_bytes();
    if key.len() > 64 {
        key_block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(key_block.map(|b| b ^ 0x36));
    inner.update(body);
    let inner_digest = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(key_block.map(|b| b ^ 0x5c));
    outer.update(inner_digest);
    outer.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn expects_of(step: &Value) -> Vec<Expect> {
    step["expect"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|e| Expect {
                    path: e["path"].as_str().expect("expect.path").to_string(),
                    value: (!e["value"].is_null()).then(|| e["value"].clone()),
                    contains: e["contains"].as_str().map(str::to_string),
                    exists: e["exists"].as_bool(),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn is_building(v: &Value) -> bool {
    v["status"] == json!("building") || v["error_code"] == json!("tool_building")
}

/// Run one recipe's golden path; panics (naming recipe, step, tool) on the
/// first failed call or `expect`, or when the run outlasts `min(60 s,
/// budget.wall_s)` (a hang-guard deadline, `deadline_budget!`).
pub async fn run(spec: &Value) -> Trace {
    let wall_s = spec["budget"]["wall_s"].as_u64().expect("budget.wall_s");
    let cap_ms = wall_s.min(MAX_RECIPE_WALL.as_secs()) * 1000;
    crate::deadline_budget!(cap_ms, { run_steps(spec).await })
}

async fn run_steps(spec: &Value) -> Trace {
    let name = spec["name"].as_str().expect("recipe name").to_string();
    let started = Instant::now();
    let envs = TempDataDir::new();
    let kinds = match spec["registry"].as_str() {
        Some("python") => python_kind_registry(&envs.0),
        _ => http_kind_registry(),
    };
    let server = if spec["https_public"].as_bool() == Some(true) {
        TestServer::start_with_kinds_https_public(kinds).await
    } else {
        TestServer::start_with_kinds(kinds).await
    };

    // Fixture routes: the in-process upstream the recipe's http tools and
    // probes talk to.
    let fixture = MockServer::start().await;
    for route in spec["fixture_routes"].as_array().cloned().unwrap_or_default() {
        let m = route["method"].as_str().unwrap_or("GET").to_string();
        let p = route["path"].as_str().expect("fixture path").to_string();
        let mut resp = ResponseTemplate::new(route["status"].as_u64().unwrap_or(200) as u16);
        resp = match &route["body"] {
            Value::Null => resp,
            Value::String(s) => resp.set_body_string(s.clone()),
            other => resp.set_body_json(other.clone()),
        };
        Mock::given(method(m.as_str())).and(path(p.as_str())).respond_with(resp).mount(&fixture).await;
    }

    let mut ctx = Ctx { tenants: HashMap::new(), fixture_base: fixture.uri(), results: Vec::new() };
    let mut clients: HashMap<String, McpClient> = HashMap::new();
    let mut issued = 0u32;
    for t in spec["tenants"].as_array().expect("tenants") {
        let t = t.as_str().expect("tenant name");
        let (ns, key) = signup(&server.base_url, &format!("rgpt {name} {t}")).await;
        clients.insert(t.to_string(), McpClient::with_bearer(&server.base_url, &key));
        ctx.tenants.insert(t.to_string(), (ns, key));
        issued += 1;
    }

    let mut calls = Vec::new();
    for (i, step) in spec["steps"].as_array().expect("steps").iter().enumerate() {
        let n = i + 1;
        if !step["signed_post"].is_null() {
            let p = &step["signed_post"];
            let url = subst(&p["url"], &ctx).as_str().expect("url").to_string();
            let secret = subst(&p["secret"], &ctx).as_str().expect("secret").to_string();
            let body = subst(&p["body"], &ctx).as_str().expect("body").to_string();
            // The server may advertise a public https origin; the hook path
            // is served by the in-process listener.
            let local = url
                .split_once("/hook/")
                .map(|(_, id)| format!("{}/hook/{id}", server.base_url))
                .unwrap_or(url);
            let mut req = reqwest::Client::new().post(local).body(body.clone());
            if p["signed"].as_bool() != Some(false) {
                req = req.header("X-Mcphost-Signature", format!("sha256={}", hmac_sha256_hex(&secret, body.as_bytes())));
            }
            let status = req.send().await.unwrap_or_else(|e| panic!("{name} step {n}: post: {e}")).status().as_u16();
            let want = p["status"].as_u64().expect("status") as u16;
            assert_eq!(status, want, "{name} step {n} (signed_post): HTTP status");
            calls.push("signed_post".to_string());
            ctx.results.push(json!({"status": status}));
            continue;
        }
        let tool = step["tool"].as_str().expect("tool").to_string();
        let who = step["as"].as_str().unwrap_or("owner");
        let client = clients.get(who).unwrap_or_else(|| panic!("{name} step {n}: no tenant `{who}`"));
        let args = subst(&step["args"], &ctx);
        let deadline = Instant::now() + Duration::from_secs(step["retry_s"].as_u64().unwrap_or(0));
        let expects = expects_of(step);
        issued += 1;
        let structured = loop {
            let outcome = client.tools_call(&tool, args.clone()).await;
            let (structured, failure) = match &outcome {
                Ok(v) => {
                    let s = extract_structured(v);
                    let failure = if v["isError"] == json!(true) {
                        Some(format!("tool error: {v}"))
                    } else if is_building(&s) {
                        Some("tool still building".to_string())
                    } else {
                        expects.iter().find_map(|e| check_expect(&s, e).err())
                    };
                    (s, failure)
                }
                Err(e) => (Value::Null, Some(format!("rpc error {} {} {:?}", e.code, e.message, e.error_code))),
            };
            match failure {
                None => break structured,
                Some(why) if Instant::now() >= deadline => {
                    panic!("{name} step {n} `{tool}` (as {who}) failed: {why}\nargs: {args}");
                }
                Some(_) => tokio::time::sleep(Duration::from_millis(200)).await,
            }
        };
        calls.push(tool);
        ctx.results.push(structured);
    }

    let elapsed = started.elapsed();
    let budget_calls = spec["budget"]["calls"].as_u64().expect("budget.calls") as u32;
    let wanted: Vec<String> = spec["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .map(|s| s["tool"].as_str().unwrap_or("signed_post").to_string())
        .collect();
    assert_eq!(calls, wanted, "{name}: steps must run in order");
    assert!(issued <= budget_calls, "{name}: issued {issued} calls, over its budget.calls {budget_calls}");
    Trace { calls, elapsed, issued }
}
