//! PRD-mcphost-recipe-golden-path-tests: `recipes/<name>.toml` is the one
//! source of a recipe. This module loads those files and renders the four
//! artifacts that used to be four hand-written copies -- the `www/llms.txt`
//! block between `<!-- recipe:<name> begin -->` / `end` markers,
//! `examples/<name>/proof.sh`, `examples/<name>/synthorg-task.yaml` and
//! `tests/rgpt_<name>_golden_path.rs`. `cargo xtask recipes` (the `xtask/`
//! bin) and the `rgpt_*` tests both call [`plan`] here, so the generator and
//! the drift check cannot disagree about what "generated" means.

use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::fmt;
use std::path::{Path, PathBuf};

/// One `[[fixture_routes]]` entry: a route the in-process wiremock fixture
/// host answers while a golden test runs.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureRoute {
    pub method: String,
    pub path: String,
    #[serde(default = "default_status")]
    pub status: u16,
    #[serde(default)]
    pub body: Value,
}

fn default_status() -> u16 {
    200
}

/// `expect`: a jsonpath into the step's structured result plus exactly one of
/// `value` (equals), `contains` (substring of the stringified value) or
/// `exists`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expect {
    pub path: String,
    pub value: Option<Value>,
    pub contains: Option<String>,
    pub exists: Option<bool>,
}

/// A sender-side action rather than a tool call: POST `body` to `url`,
/// signed (HMAC-SHA256 under `secret`, `X-Mcphost-Signature`) unless
/// `signed = false`, and expect `status`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedPost {
    pub url: String,
    #[serde(rename = "secret")]
    pub signing_key: String,
    pub body: String,
    #[serde(default = "default_true")]
    pub signed: bool,
    pub status: u16,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub tool: Option<String>,
    #[serde(rename = "as", default = "default_tenant")]
    pub as_tenant: String,
    #[serde(default)]
    pub args: Value,
    #[serde(default)]
    pub expect: Vec<Expect>,
    pub note: Option<String>,
    /// Re-issue the call for up to this many seconds while it errors (a
    /// python tool's first call may still be building its environment).
    /// With `retry_until_expect` the same budget also covers a call that
    /// succeeds but whose `expect`s do not hold yet.
    pub retry_s: Option<u64>,
    /// Opt in (requires `retry_s`): re-issue the call while it errors OR any
    /// `expect` fails, until the `retry_s` deadline, then fail with the LAST
    /// expect failure text (never a bare timeout). The poll interval is
    /// [`RETRY_UNTIL_EXPECT_POLL_S`]; the generated proof defines
    /// `RETRY_UNTIL_EXPECT_POLL_S` for the body to sleep on, and the body must
    /// use it. For a result another component produces on its own tick (a
    /// sync receipt), where the first answer is legitimately not ready.
    #[serde(default)]
    pub retry_until_expect: bool,
    /// Replaces the call text of this step in the `www/llms.txt` block only
    /// (the golden test and the proof still use `args`). For a call whose
    /// literal args would repeat a token the contract tests count once in
    /// `www/llms.txt` (a source kind, syreg_ac02) or a URL the docs link
    /// check would fetch.
    pub llms_call: Option<String>,
    pub signed_post: Option<SignedPost>,
}

/// Seconds between re-issues inside a `retry_until_expect` step (a tick-driven
/// result moves at tick granularity, so polling faster only adds calls).
pub const RETRY_UNTIL_EXPECT_POLL_S: u64 = 5;

/// `tests/support/rgpt.rs` `MAX_RECIPE_WALL`: the in-process runner kills any
/// recipe at this many seconds whatever its `wall_s`. A test reads the real
/// constant out of that file and fails if the two differ.
pub const RGPT_MAX_RECIPE_WALL_S: u64 = 60;
/// Seconds the non-polling steps of a recipe take (measured: the data-sync
/// proof's signup, create, run_now, list and delete ran in ~2 s on the
/// in-process server).
pub const RECIPE_NON_POLL_STEP_S: u64 = 2;
/// Headroom kept under [`RGPT_MAX_RECIPE_WALL_S`] in the worst case.
pub const RECIPE_WALL_MARGIN_S: u64 = 5;

/// Worst-case seconds of a recipe with `retry_until_expect` steps: every such
/// step runs its whole `retry_s` plus one poll of overshoot (the loop checks
/// the deadline, then sleeps), plus the non-polling steps. 0 without such steps.
pub fn retry_until_expect_worst_case_s(r: &Recipe) -> u64 {
    let polled: Vec<u64> = r.steps.iter().filter(|s| s.retry_until_expect).map(|s| s.retry_s.unwrap_or(0)).collect();
    if polled.is_empty() {
        return 0;
    }
    polled.iter().map(|t| t + RETRY_UNTIL_EXPECT_POLL_S).sum::<u64>() + RECIPE_NON_POLL_STEP_S
}

fn default_tenant() -> String {
    "owner".to_string()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Budget {
    pub calls: u32,
    pub wall_s: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Gold {
    pub tool_name: String,
    pub call_check: String,
    pub kind: String,
}

/// The synthorg consumer task, rendered verbatim into
/// `examples/<name>/synthorg-task.yaml`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub header: String,
    pub id: String,
    pub segments: Vec<String>,
    /// The folded block under `task: >`, lines already indented.
    pub text: String,
    pub gold: Gold,
    pub fixture_route: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proof {
    /// Everything after the generated banner (starts at the script's own
    /// header comment, ends with a newline).
    pub body: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    #[serde(skip)]
    pub name: String,
    pub title: String,
    /// The `## ` heading of the hand-written section the generated block
    /// lives in.
    pub llms_section: String,
    /// First generation only. Default: the block goes immediately before the
    /// section's `## ` heading, so the section's own tests (which slice from
    /// the heading) never see it. Set it to a line of the section when the
    /// heading sits inside another generated region (docs-qa is inside the
    /// agent-quickstart block): the block goes after that line instead.
    pub llms_insert_after: Option<String>,
    pub plan_fit: String,
    /// `"http"` or `"python"`: which kind registry the golden test boots.
    pub registry: String,
    #[serde(default)]
    pub https_public: bool,
    #[serde(default = "default_tenants")]
    pub tenants: Vec<String>,
    #[serde(default)]
    pub fixture_routes: Vec<FixtureRoute>,
    pub steps: Vec<Step>,
    pub budget: Budget,
    pub task: Task,
    pub proof: Proof,
}

fn default_tenants() -> Vec<String> {
    vec!["owner".to_string()]
}

#[derive(Debug)]
pub enum RecipeError {
    /// A bad step: names recipe, 1-based step index and field (exit 2).
    Invalid { recipe: String, step: usize, field: String, msg: String },
    /// Anything else (unreadable file, unparsable toml, missing anchor).
    Other(String),
}

impl RecipeError {
    pub fn exit_code(&self) -> i32 {
        match self {
            RecipeError::Invalid { .. } => 2,
            RecipeError::Other(_) => 1,
        }
    }
}

impl fmt::Display for RecipeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RecipeError::Invalid { recipe, step, field, msg } => {
                write!(f, "recipe {recipe}: step {step}: field {field}: {msg}")
            }
            RecipeError::Other(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for RecipeError {}

fn other<T: fmt::Display>(m: T) -> RecipeError {
    RecipeError::Other(m.to_string())
}

/// Load every `recipes/*.toml` under `root`, sorted by file name.
pub fn load_all(root: &Path) -> Result<Vec<Recipe>, RecipeError> {
    let dir = root.join("recipes");
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map_err(|e| other(format!("read {}: {e}", dir.display())))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    paths.sort();
    let mut out = Vec::new();
    for p in paths {
        let text = std::fs::read_to_string(&p).map_err(|e| other(format!("read {}: {e}", p.display())))?;
        let mut r: Recipe = toml::from_str(&text).map_err(|e| other(format!("{}: {e}", p.display())))?;
        r.name = p.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string();
        out.push(r);
    }
    Ok(out)
}

// ---- validation (R3) ---------------------------------------------------

/// R3: every step's `tool` must be a name in the keyed `tools/list`
/// (`handler::host_tool_descriptors`, the set `llms_txt_tool_names` is built
/// from) and every `args` key a property of that tool's input schema.
pub fn validate(recipes: &[Recipe]) -> Result<(), RecipeError> {
    let tools = crate::handler::host_tool_descriptors(&crate::kinds::KindRegistry::with_builtin());
    for r in recipes {
        let invalid = |step: usize, field: &str, msg: String| RecipeError::Invalid {
            recipe: r.name.clone(),
            step,
            field: field.to_string(),
            msg,
        };
        if r.steps.is_empty() {
            return Err(invalid(0, "steps", "a recipe needs at least one step".to_string()));
        }
        for (i, s) in r.steps.iter().enumerate() {
            let n = i + 1;
            if !r.tenants.contains(&s.as_tenant) {
                return Err(invalid(n, "as", format!("`{}` is not in tenants {:?}", s.as_tenant, r.tenants)));
            }
            if s.llms_call.as_deref().is_some_and(|t| t.trim().is_empty()) {
                return Err(invalid(n, "llms_call", "`llms_call` must not be empty (omit the key to render the call from `args`)".to_string()));
            }
            if s.retry_until_expect {
                if s.retry_s.is_none() {
                    return Err(invalid(n, "retry_until_expect", "`retry_until_expect` requires `retry_s`".to_string()));
                }
                let deadlines: std::collections::BTreeSet<u64> =
                    r.steps.iter().filter(|x| x.retry_until_expect).filter_map(|x| x.retry_s).collect();
                if deadlines.len() > 1 {
                    return Err(invalid(
                        n,
                        "retry_s",
                        format!("retry_until_expect steps must share one retry_s (the proof has one deadline variable), got {deadlines:?}"),
                    ));
                }
                let worst = retry_until_expect_worst_case_s(r);
                if worst + RECIPE_WALL_MARGIN_S > RGPT_MAX_RECIPE_WALL_S {
                    return Err(invalid(
                        n,
                        "retry_s",
                        format!(
                            "worst case {worst} s (retry_s + {RETRY_UNTIL_EXPECT_POLL_S} s poll + {RECIPE_NON_POLL_STEP_S} s other steps) leaves under {RECIPE_WALL_MARGIN_S} s below the {RGPT_MAX_RECIPE_WALL_S} s in-process cap"
                        ),
                    ));
                }
                let deadline = s.retry_s.unwrap_or(0).to_string();
                if r.proof.body.split(|c: char| !c.is_ascii_digit()).any(|t| t == deadline) {
                    return Err(invalid(
                        n,
                        "retry_s",
                        format!("the proof body hard-codes the deadline {deadline}; use $RETRY_UNTIL_EXPECT_DEADLINE_S (the generator defines it)"),
                    ));
                }
                if !r.proof.body.contains("RETRY_UNTIL_EXPECT_DEADLINE_S") {
                    return Err(invalid(
                        n,
                        "retry_until_expect",
                        "the proof body must use $RETRY_UNTIL_EXPECT_DEADLINE_S (the generator defines it)".to_string(),
                    ));
                }
                if !r.proof.body.contains("RETRY_UNTIL_EXPECT_POLL_S") {
                    return Err(invalid(
                        n,
                        "retry_until_expect",
                        "the proof body must poll with $RETRY_UNTIL_EXPECT_POLL_S (the generator defines it)".to_string(),
                    ));
                }
            }
            let Some(name) = s.tool.as_deref() else {
                if s.signed_post.is_none() {
                    return Err(invalid(n, "tool", "a step needs `tool` or `signed_post`".to_string()));
                }
                continue;
            };
            let Some(tool) = tools.iter().find(|t| t.name.as_ref() == name) else {
                return Err(invalid(n, "tool", format!("`{name}` is not in tools/list")));
            };
            let args = match &s.args {
                Value::Null => Map::new(),
                // A whole-value reference (`"${s1.wait.args}"`) is only known at run time.
                Value::String(t) if t.starts_with("${") => continue,
                Value::Object(m) => m.clone(),
                _ => return Err(invalid(n, "args", "args must be a table".to_string())),
            };
            let props = tool.input_schema.get("properties").and_then(Value::as_object);
            if let Some(props) = props {
                for key in args.keys() {
                    if !props.contains_key(key) {
                        return Err(invalid(
                            n,
                            &format!("args.{key}"),
                            format!("`{key}` is not in {name}'s input schema"),
                        ));
                    }
                }
            }
            if let Some(req) = tool.input_schema.get("required").and_then(Value::as_array) {
                for key in req.iter().filter_map(Value::as_str) {
                    if !args.contains_key(key) {
                        return Err(invalid(
                            n,
                            &format!("args.{key}"),
                            format!("`{key}` is required by {name}'s input schema"),
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

// ---- jsonpath + expect (R1, R4) ----------------------------------------

/// `$.a.b[0].c` -> the value at that path, or `None`.
pub fn path_get<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    let rest = path.strip_prefix('$')?;
    let mut cur = root;
    let bytes = rest.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'.' => {
                let start = i + 1;
                let mut end = start;
                while end < bytes.len() && bytes[end] != b'.' && bytes[end] != b'[' {
                    end += 1;
                }
                cur = cur.get(&rest[start..end])?;
                i = end;
            }
            b'[' => {
                let close = rest[i..].find(']')? + i;
                let idx: usize = rest[i + 1..close].parse().ok()?;
                cur = cur.get(idx)?;
                i = close + 1;
            }
            _ => return None,
        }
    }
    Some(cur)
}

/// `Err(why)` when `result` does not satisfy the expectation.
pub fn check_expect(result: &Value, e: &Expect) -> Result<(), String> {
    let got = path_get(result, &e.path);
    if let Some(want) = e.exists {
        return if got.is_some() == want {
            Ok(())
        } else {
            Err(format!("{}: exists should be {want}, result {result}", e.path))
        };
    }
    let Some(got) = got else {
        return Err(format!("{}: missing in result {result}", e.path));
    };
    if let Some(want) = &e.value
        && got != want
    {
        return Err(format!("{}: want {want}, got {got}", e.path));
    }
    if let Some(sub) = &e.contains {
        let text = got.as_str().map_or_else(|| got.to_string(), str::to_string);
        if !text.contains(sub.as_str()) {
            return Err(format!("{}: want to contain {sub:?}, got {got}", e.path));
        }
    }
    Ok(())
}

// ---- rendering ---------------------------------------------------------

/// A template string for a human: `${owner.ns}` -> `<owner_namespace>`,
/// `${s3.url}` -> `<step3.url>`, `${file:p}` -> `<contents of p>`.
fn humanize(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('}') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let inner = &rest[start + 2..start + end];
        let text = if let Some(p) = inner.strip_prefix("file:") {
            format!("<contents of {p}>")
        } else if let Some(t) = inner.strip_suffix(".ns") {
            format!("<{t}_namespace>")
        } else if let Some(t) = inner.strip_suffix(".key") {
            format!("<{t}_key>")
        } else if let Some(p) = inner.strip_prefix('s').filter(|p| p.starts_with(|c: char| c.is_ascii_digit())) {
            format!("<step{p}>")
        } else {
            format!("<{inner}>")
        };
        out.push_str(&text);
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    out
}

fn humanize_value(v: &Value) -> Value {
    match v {
        Value::String(s) => Value::String(humanize(s)),
        Value::Array(a) => Value::Array(a.iter().map(humanize_value).collect()),
        Value::Object(m) => Value::Object(m.iter().map(|(k, v)| (k.clone(), humanize_value(v))).collect()),
        other => other.clone(),
    }
}

fn render_call(s: &Step) -> String {
    if let Some(p) = &s.signed_post {
        let how = if p.signed {
            "signed (HMAC-SHA256 under the secret, `X-Mcphost-Signature: sha256=<hex>`)"
        } else {
            "unsigned"
        };
        return format!("POST `{}` to `{}`, {how}, expect HTTP {}", humanize(&p.body), humanize(&p.url), p.status);
    }
    if let Some(text) = &s.llms_call {
        return text.clone();
    }
    let tool = s.tool.as_deref().unwrap_or_default();
    let args = match humanize_value(&s.args) {
        Value::Object(m) => m
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(", "),
        _ => String::new(),
    };
    format!("`{tool}({args})`")
}

pub fn begin_marker(name: &str) -> String {
    format!("<!-- recipe:{name} begin -->")
}

pub fn end_marker(name: &str) -> String {
    format!("<!-- recipe:{name} end -->")
}

/// The generated `www/llms.txt` block, markers included (no trailing newline).
pub fn render_llms_block(r: &Recipe) -> String {
    let mut out = String::new();
    out.push_str(&begin_marker(&r.name));
    out.push_str(&format!(
        "\n## Recipe calls: {}\n\nThe exact calls, in order (generated from `recipes/{}.toml`; do not hand-edit -- run `cargo xtask recipes`). Plan fit: {}.\n\n",
        r.title, r.name, r.plan_fit
    ));
    let signups = r.tenants.len();
    out.push_str(&format!(
        "Sign up first (`signup`), once per tenant: {}.\n\n",
        r.tenants.iter().map(|t| format!("`{t}`")).collect::<Vec<_>>().join(", ")
    ));
    for (i, s) in r.steps.iter().enumerate() {
        let who = if r.tenants.len() > 1 { format!(" [{}]", s.as_tenant) } else { String::new() };
        out.push_str(&format!("{}. {}{who}", i + 1, render_call(s)));
        if let Some(note) = &s.note {
            out.push_str(&format!(" -- {note}"));
        }
        out.push('\n');
        for e in &s.expect {
            let want = match (&e.value, &e.contains, e.exists) {
                (Some(v), _, _) => format!("= {v}"),
                (_, Some(c), _) => format!("contains {c:?}"),
                (_, _, Some(true)) => "is present".to_string(),
                (_, _, Some(false)) => "is absent".to_string(),
                _ => "is present".to_string(),
            };
            out.push_str(&format!("   expect `{}` {want}\n", e.path));
        }
    }
    out.push_str(&format!(
        "\nExpected cost: {} calls (the {signups} signup{} included), within {} s.\n",
        r.budget.calls,
        if signups == 1 { "" } else { "s" },
        r.budget.wall_s
    ));
    out.push_str(&end_marker(&r.name));
    out
}

/// Insert or replace `r`'s block in `llms`.
pub fn apply_llms(llms: &str, r: &Recipe) -> Result<String, RecipeError> {
    let block = render_llms_block(r);
    let (begin, end) = (begin_marker(&r.name), end_marker(&r.name));
    if let Some(b) = llms.find(&begin) {
        let e = llms[b..]
            .find(&end)
            .map(|e| e + b + end.len())
            .ok_or_else(|| other(format!("www/llms.txt: `{begin}` has no matching end marker")))?;
        return Ok(format!("{}{}{}", &llms[..b], block, &llms[e..]));
    }
    let heading = format!("## {}", r.llms_section);
    let lines: Vec<&str> = llms.split('\n').collect();
    let h = lines
        .iter()
        .position(|l| *l == heading)
        .ok_or_else(|| other(format!("recipe {}: www/llms.txt has no `{heading}` section", r.name)))?;
    // `at` = index of the line the block goes after; `None` = at the very top.
    let after: Option<usize> = match &r.llms_insert_after {
        Some(anchor) => {
            let section_end = lines[h + 1..].iter().position(|l| l.starts_with("## ")).map_or(lines.len(), |p| p + h + 1);
            let at = lines[h..section_end]
                .iter()
                .position(|l| l.contains(anchor.as_str()))
                .map(|p| p + h)
                .ok_or_else(|| {
                    other(format!("recipe {}: `{anchor}` not found inside section `{heading}`", r.name))
                })?;
            Some(at)
        }
        None => {
            // Before the heading, and before any marker comment sitting
            // directly on top of it (`<!-- x:start -->` opens a region the
            // block must stay outside).
            let mut top = h;
            while top > 0 && lines[top - 1].starts_with("<!--") && lines[top - 1].ends_with("-->") {
                top -= 1;
            }
            top.checked_sub(1)
        }
    };
    let keep = after.map_or(0, |a| a + 1);
    let mut out: Vec<String> = lines[..keep].iter().map(|l| l.to_string()).collect();
    if out.last().is_some_and(|l| !l.is_empty()) {
        out.push(String::new());
    }
    out.extend(block.split('\n').map(str::to_string));
    if lines.get(keep).is_some_and(|l| !l.is_empty()) {
        out.push(String::new());
    }
    out.extend(lines[keep..].iter().map(|l| l.to_string()));
    Ok(out.join("\n"))
}

/// `www/llms.txt` with `r`'s block (and the blank line before it) removed.
pub fn strip_llms_block(llms: &str, name: &str) -> String {
    let (begin, end) = (begin_marker(name), end_marker(name));
    let Some(b) = llms.find(&begin) else { return llms.to_string() };
    let Some(e) = llms[b..].find(&end).map(|e| e + b + end.len()) else { return llms.to_string() };
    let head = llms[..b].strip_suffix('\n').unwrap_or(&llms[..b]);
    let tail = llms[e..].strip_prefix('\n').unwrap_or(&llms[e..]);
    format!("{head}{tail}")
}

pub fn render_proof(r: &Recipe) -> String {
    let mut out = String::from("#!/usr/bin/env bash\n");
    out.push_str(&format!(
        "# GENERATED from recipes/{}.toml by `cargo xtask recipes` -- do not hand-edit.\n",
        r.name
    ));
    out.push_str(&format!("# Recipe calls (budget: {} calls, within {} s):\n", r.budget.calls, r.budget.wall_s));
    for (i, s) in r.steps.iter().enumerate() {
        let what = s.tool.clone().unwrap_or_else(|| "signed POST to the returned url".to_string());
        out.push_str(&format!("#   {}. {what}\n", i + 1));
    }
    let mut body = with_prove_dry(&r.name, &r.proof.body);
    if let Some(deadline) = r.steps.iter().find(|s| s.retry_until_expect).and_then(|s| s.retry_s) {
        body = with_poll_const(&body, deadline);
    }
    out.push_str(&body);
    out
}

/// Defines `RETRY_UNTIL_EXPECT_POLL_S` (the [`RETRY_UNTIL_EXPECT_POLL_S`]
/// constant) and `RETRY_UNTIL_EXPECT_DEADLINE_S` (the step's `retry_s`) right after the dry-run block, so a body that polls a
/// `retry_until_expect` step cannot drift from the value the golden runner
/// and the docs use.
fn with_poll_const(body: &str, deadline_s: u64) -> String {
    let marker = "exit 0; }\n";
    let Some(at) = body.find(marker).map(|i| i + marker.len()) else { return body.to_string() };
    format!(
        "{}# Seconds between re-issues, and the step's retry_s, of a retry_until_expect step (generated).\nRETRY_UNTIL_EXPECT_POLL_S={}\nRETRY_UNTIL_EXPECT_DEADLINE_S={}\n{}",
        &body[..at],
        RETRY_UNTIL_EXPECT_POLL_S,
        deadline_s,
        &body[at..]
    )
}

/// The PRD-mcphost-example-proof-runner-contract dry-run block: every proof
/// prints `plan: <recipe> proof against $MCPHOST_URL` and exits 0 before any
/// network call when `MCPHOST_PROVE_DRY` is set (the runner-contract test,
/// `eprc_ac02`, sets it). Rendered once here, directly after the proof
/// body's `MCPHOST_URL=` default line, so no recipe toml carries a copy.
fn with_prove_dry(name: &str, body: &str) -> String {
    let mut out = String::with_capacity(body.len() + 320);
    let mut done = false;
    for line in body.split_inclusive('\n') {
        out.push_str(line);
        if !done && line.starts_with("MCPHOST_URL=") {
            out.push_str(
                "# MCPHOST_PROVE_DRY=1: print the plan and exit 0 before any network call (the runner-contract test sets it).\n",
            );
            out.push_str(&format!(
                "[[ -n \"${{MCPHOST_PROVE_DRY:-}}\" ]] && {{ echo \"plan: {name} proof against $MCPHOST_URL\"; exit 0; }}\n"
            ));
            done = true;
        }
    }
    out
}

pub fn render_task_yaml(r: &Recipe) -> String {
    let t = &r.task;
    let mut out = String::new();
    out.push_str(&t.header);
    if !t.header.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("tasks:\n");
    out.push_str(&format!("  - id: {}\n", t.id));
    out.push_str(&format!("    segments: [{}]\n", t.segments.join(", ")));
    out.push_str("    task: >\n");
    out.push_str(&t.text);
    if !t.text.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("    gold:\n");
    out.push_str(&format!("      tool_name: {}\n", t.gold.tool_name));
    out.push_str(&format!("      call_check: \"{}\"\n", t.gold.call_check));
    out.push_str(&format!("      kind: {}\n", t.gold.kind));
    if let Some(route) = &t.fixture_route {
        out.push_str(&format!("    fixture_route: {route}\n"));
    }
    out.push_str(&format!("    recipe: {}\n", r.name));
    out.push_str(&format!("    wall_budget_s: {}\n", r.budget.wall_s));
    out
}

/// The JSON the golden test embeds and `tests/support/rgpt.rs` runs.
pub fn golden_spec(r: &Recipe) -> Value {
    json!({
        "name": r.name,
        "registry": r.registry,
        "https_public": r.https_public,
        "tenants": r.tenants,
        "fixture_routes": r.fixture_routes.iter().map(|f| json!({
            "method": f.method, "path": f.path, "status": f.status, "body": f.body,
        })).collect::<Vec<_>>(),
        "steps": r.steps.iter().map(|s| json!({
            "tool": s.tool,
            "as": s.as_tenant,
            "args": if s.args.is_null() { json!({}) } else { s.args.clone() },
            "expect": s.expect.iter().map(|e| json!({
                "path": e.path, "value": e.value, "contains": e.contains, "exists": e.exists,
            })).collect::<Vec<_>>(),
            "retry_s": s.retry_s,
            "signed_post": s.signed_post.as_ref().map(|p| json!({
                "url": p.url, "secret": p.signing_key, "body": p.body, "signed": p.signed, "status": p.status,
            })),
        })).enumerate().map(|(i, mut v)| {
            // Only an opted-in step carries the keys, so every other recipe's
            // embedded spec (and generated test) stays byte-identical.
            if r.steps[i].retry_until_expect {
                v["retry_until_expect"] = json!(true);
                v["poll_s"] = json!(RETRY_UNTIL_EXPECT_POLL_S);
            }
            v
        }).collect::<Vec<_>>(),
        "budget": {"calls": r.budget.calls, "wall_s": r.budget.wall_s},
    })
}

pub fn render_golden_test(r: &Recipe) -> Result<String, RecipeError> {
    let spec = serde_json::to_string_pretty(&golden_spec(r)).map_err(other)?;
    if spec.contains("\"####") {
        return Err(other(format!("recipe {}: a step contains the raw-string terminator", r.name)));
    }
    let python = r.registry == "python";
    let mut out = String::new();
    out.push_str(&format!(
        "//! GENERATED from recipes/{n}.toml by `cargo xtask recipes` -- do not hand-edit.\n\
         //! PRD-mcphost-recipe-golden-path-tests R4: the `{n}` recipe's exact calls, in\n\
         //! order, against an in-process `TestServer` with the recipe's fixture routes\n\
         //! mounted; every `expect` must hold and the run must finish under {w} s.\n{cap}\n",
        n = r.name,
        // The runner enforces min(wall_s, MAX_RECIPE_WALL), so a 90 s recipe is
        // still held to the cap.
        w = u64::from(r.budget.wall_s).min(RGPT_MAX_RECIPE_WALL_S),
        cap = if r.steps.iter().any(|s| s.retry_until_expect) {
            format!(
                "//! This recipe polls (retry_until_expect): wall_s is {wall} s, but the in-process runner\n\
                 //! caps every recipe at MAX_RECIPE_WALL = {RGPT_MAX_RECIPE_WALL_S} s (tests/support/rgpt.rs), so the polling\n\
                 //! worst case ({worst} s) is kept under that cap, not under wall_s.\n",
                wall = r.budget.wall_s,
                worst = retry_until_expect_worst_case_s(r),
            )
        } else {
            String::new()
        },
    ));
    out.push_str("use crate::rgpt;\n\n");
    out.push_str(&format!("const RECIPE: &str = r####\"{spec}\"####;\n\n"));
    out.push_str("#[tokio::test]\n");
    out.push_str(&format!("async fn rgpt_{}_golden_path() {{\n", r.name.replace('-', "_")));
    if python {
        out.push_str(
            "    if mcphost::sandbox::require_user_namespaces_or_ci_skip() {\n        \
             println!(\"{} (CI)\", mcphost::sandbox::USERNS_SKIP_MARKER);\n        return;\n    }\n",
        );
    }
    out.push_str("    let spec: serde_json::Value = serde_json::from_str(RECIPE).unwrap_or_else(|e| panic!(\"embedded recipe json: {e}\"));\n");
    out.push_str("    rgpt::run(&spec).await;\n}\n");
    Ok(out)
}

// ---- plan / write / check ----------------------------------------------

/// Every generated file as `(path relative to root, content)`. `llms` is the
/// current `www/llms.txt`; the returned llms.txt content has every block
/// inserted or refreshed.
pub fn plan(root: &Path) -> Result<Vec<(PathBuf, String)>, RecipeError> {
    let recipes = load_all(root)?;
    validate(&recipes)?;
    plan_for(root, &recipes)
}

pub fn plan_for(root: &Path, recipes: &[Recipe]) -> Result<Vec<(PathBuf, String)>, RecipeError> {
    let llms_path = root.join("www/llms.txt");
    let mut llms = std::fs::read_to_string(&llms_path)
        .map_err(|e| other(format!("read {}: {e}", llms_path.display())))?;
    let mut files = Vec::new();
    for r in recipes {
        llms = apply_llms(&llms, r)?;
        files.push((PathBuf::from(format!("examples/{}/proof.sh", r.name)), render_proof(r)));
        files.push((PathBuf::from(format!("examples/{}/synthorg-task.yaml", r.name)), render_task_yaml(r)));
        files.push((
            PathBuf::from(format!("tests/rgpt_{}_golden_path.rs", r.name.replace('-', "_"))),
            render_golden_test(r)?,
        ));
    }
    files.push((PathBuf::from("www/llms.txt"), llms));
    Ok(files)
}

/// Write every planned file whose bytes differ; returns the changed paths.
pub fn write(root: &Path, files: &[(PathBuf, String)]) -> Result<Vec<PathBuf>, RecipeError> {
    let mut changed = Vec::new();
    for (rel, content) in files {
        let path = root.join(rel);
        if std::fs::read_to_string(&path).ok().as_deref() == Some(content.as_str()) {
            continue;
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| other(format!("mkdir {}: {e}", dir.display())))?;
        }
        std::fs::write(&path, content).map_err(|e| other(format!("write {}: {e}", path.display())))?;
        changed.push(rel.clone());
    }
    Ok(changed)
}

/// First line (1-based) where `a` and `b` differ, with both lines.
pub fn first_diff(a: &str, b: &str) -> Option<(usize, String, String)> {
    let (mut la, mut lb) = (a.split('\n'), b.split('\n'));
    let mut n = 0;
    loop {
        n += 1;
        match (la.next(), lb.next()) {
            (None, None) => return None,
            (x, y) if x == y => {}
            (x, y) => return Some((n, x.unwrap_or("<end of file>").to_string(), y.unwrap_or("<end of file>").to_string())),
        }
    }
}

/// R5: one message per drifted file, naming the file, the recipe (when the
/// file belongs to one) and the first differing line.
pub fn drift(root: &Path) -> Result<Vec<String>, RecipeError> {
    let recipes = load_all(root)?;
    let files = plan(root)?;
    let mut out = Vec::new();
    for (rel, want) in &files {
        let have = std::fs::read_to_string(root.join(rel)).unwrap_or_default();
        let Some((line, have_l, want_l)) = first_diff(&have, want) else { continue };
        let owner = if rel.as_path() == Path::new("www/llms.txt") {
            recipes
                .iter()
                .find(|r| {
                    let block = render_llms_block(r);
                    !have.contains(&block)
                })
                .map(|r| r.name.clone())
                .unwrap_or_else(|| "-".to_string())
        } else {
            recipes
                .iter()
                .find(|r| rel.to_string_lossy().contains(&format!("/{}/", r.name))
                    || rel.to_string_lossy().contains(&format!("rgpt_{}_", r.name.replace('-', "_"))))
                .map(|r| r.name.clone())
                .unwrap_or_else(|| "-".to_string())
        };
        out.push(format!(
            "{}: recipe {owner}: line {line} drifted from recipes/{owner}.toml: tree has {have_l:?}, generator wants {want_l:?}",
            rel.display()
        ));
    }
    Ok(out)
}

/// `cargo xtask recipes [--check]`. Returns the process exit code.
pub fn cli(root: &Path, args: &[String]) -> i32 {
    let check = args.iter().any(|a| a == "--check");
    if check {
        return match drift(root) {
            Ok(d) if d.is_empty() => 0,
            Ok(d) => {
                for m in d {
                    eprintln!("{m}");
                }
                1
            }
            Err(e) => {
                eprintln!("{e}");
                e.exit_code()
            }
        };
    }
    match plan(root).and_then(|files| write(root, &files)) {
        Ok(changed) => {
            for c in &changed {
                println!("wrote {}", c.display());
            }
            println!("recipes: {} file(s) changed", changed.len());
            0
        }
        Err(e) => {
            eprintln!("{e}");
            e.exit_code()
        }
    }
}
