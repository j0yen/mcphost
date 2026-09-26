//! PRD-mcphost-run-result-overflow-to-state
//! AC7 (P0) — Given `www/llms.txt`, When the "Long-running jobs" section is
//! followed literally by this test (each call parsed and executed against
//! the test host with a 1.4 MiB fixture tool), Then every call succeeds
//! and the final read returns `items_processed`.
//!
//! The doc's own walk-through writes `spec={"source": "..."}` (source
//! omitted for space, same convention `www/llms.txt`'s other walk-throughs
//! already use -- see "Give your agents one memory") -- this test is the
//! one place that "..." is substituted for a real 1.4 MiB fixture tool
//! before executing, so the doc stays literally executable without
//! actually inlining a python source into it.

use crate::common;
use common::{TestServer, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::{Map, Value, json};
use std::time::Duration;

const LLMS_TXT: &str = include_str!("../www/llms.txt");

const FIXTURE_SOURCE: &str =
    "def main(args):\n    return {\"blob\": \"x\" * args.get(\"n\", 0)}\n";

/// Splits `inner` on top-level commas -- respecting `{}`/`[]` nesting and
/// `"..."` quoting, so a comma inside a nested object/array/string never
/// splits a `key=value` pair in half.
fn split_top_level(inner: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut in_str = false;
    let bytes = inner.as_bytes();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if in_str {
            if c == '\\' {
                i += 1;
            } else if c == '"' {
                in_str = false;
            }
        } else {
            match c {
                '"' => in_str = true,
                '{' | '[' => depth += 1,
                '}' | ']' => depth -= 1,
                ',' if depth == 0 => {
                    parts.push(inner[start..i].trim());
                    start = i + 1;
                }
                _ => {}
            }
        }
        i += 1;
    }
    if start < inner.len() {
        parts.push(inner[start..].trim());
    }
    parts.into_iter().filter(|p| !p.is_empty()).collect()
}

/// Parses one doc call line (`tool.name(key="value", key2={...}, ...)`)
/// into `(tool_name, args_object)` -- every `key=value` segment's `value`
/// is already valid JSON text in this doc's own convention (quoted
/// strings, numbers, booleans, `{"a": 1}`-shaped objects), so this only
/// has to turn `key=value` kwargs syntax into a JSON object, never
/// reinterpret the values themselves.
fn parse_call(line: &str) -> (String, Value) {
    let line = line.trim();
    let open = line.find('(').unwrap_or_else(|| panic!("call has no '(': {line}"));
    assert!(line.ends_with(')'), "call must end with ')': {line}");
    let name = line[..open].to_string();
    let inner = &line[open + 1..line.len() - 1];
    let mut obj = Map::new();
    for pair in split_top_level(inner) {
        let eq = pair.find('=').unwrap_or_else(|| panic!("malformed key=value pair: {pair}"));
        let key = pair[..eq].trim();
        let value_str = pair[eq + 1..].trim();
        let value: Value = serde_json::from_str(value_str)
            .unwrap_or_else(|e| panic!("bad JSON value for '{key}' in {line:?}: {value_str:?}: {e}"));
        obj.insert(key.to_string(), value);
    }
    (name, Value::Object(obj))
}

/// The walk-through's own fenced code block, immediately after "Walk-through"
/// inside the "## Long-running jobs" section.
fn walkthrough_lines() -> Vec<String> {
    let section_start = LLMS_TXT
        .find("## Long-running jobs")
        .expect("www/llms.txt must have a '## Long-running jobs' section");
    let section = &LLMS_TXT[section_start..];
    let fence_start = section.find("```").expect("Long-running jobs section must have a fenced walk-through");
    let after_fence = &section[fence_start + 3..];
    let fence_end = after_fence.find("```").expect("walk-through fence must close");
    after_fence[..fence_end]
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

#[tokio::test]
async fn long_running_jobs_walkthrough_executes_literally_against_a_1_4mib_fixture() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let lines = walkthrough_lines();
    assert!(lines.len() >= 4, "walk-through must have several calls: {lines:?}");

    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "RunOverflow AC7 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let mut run_id: Option<String> = None;
    let mut last_wait: Option<Value> = None;
    for raw_line in &lines {
        let line = match &run_id {
            Some(id) => raw_line.replace("<run_id>", id),
            None => raw_line.clone(),
        };
        let (name, mut args) = parse_call(&line);

        // The doc's own "source omitted for space" convention -- substitute
        // the real fixture tool so this call is genuinely executable.
        if name == "host.tool_publish"
            && let Some(spec) = args.get_mut("spec")
            && spec.get("source") == Some(&json!("..."))
        {
            spec["source"] = json!(FIXTURE_SOURCE);
            spec["args_schema"] = json!({"type": "object"});
        }

        let result = if name == "host.tool_publish" {
            // Cold-build through a dry run first so the async call below
            // measures the job path, not the one-time environment build.
            client.tools_call(&name, args.clone()).await.map(|v| {
                let local_name = args["name"].as_str().unwrap().to_string();
                let qualified = format!("{ns}.{local_name}");
                (v, Some(qualified))
            })
        } else {
            client.tools_call(&name, args.clone()).await.map(|v| (v, None))
        };
        let (value, qualified) = result.unwrap_or_else(|e| {
            panic!("walk-through call {name}({args}) must succeed: {} {}", e.code, e.message)
        });

        if let Some(qualified) = qualified {
            let _ = poll_until_ready(&client, &qualified, json!({"n": 1}), Duration::from_secs(15))
                .await
                .expect("warm-up call ok");
        }

        let structured = common::extract_structured(&value);
        if name == "host.tool_call"
            && let Some(id) = structured.get("run_id").and_then(Value::as_str)
        {
            run_id = Some(id.to_string());
        }
        if name == "host.runs.wait" {
            last_wait = Some(structured);
        }
    }

    let last_wait = last_wait.expect("walk-through must include a host.runs.wait call");
    let items_processed = last_wait["counters"]["items_processed"]
        .as_i64()
        .unwrap_or_else(|| panic!("final read must return items_processed: {last_wait}"));
    assert_eq!(items_processed, 1000, "final read: {last_wait}");
}
