//! `mcphost-qjs`: the QuickJS runner the `js` kind (`src/kinds/js.rs`) runs
//! inside the python kind's bwrap profile.
//!
//! Usage: `mcphost-qjs [--check] [--memory-mb N] [--timeout-ms N] [--network
//! none|public] [--prelude FILE] <tool.js>`.
//!
//! Normal mode speaks the same line protocol the python runner does: one
//! JSON request per stdin line (`{"args": ..., "end_user": ...}`), one JSON
//! envelope per stdout line (`{"ok": true, "result": ...}` or `{"ok": false,
//! "kind": "exception"|"oom"|"timeout"|"output_invalid", ...}`). Mid-call
//! `mcphost.*` requests go out as marker lines on the same stdout and block
//! on the next stdin line, exactly like python's `_state_call`; the host side
//! is `kinds::python::HostSidecarBridge`. The `mcphost` object itself comes
//! from the prelude the host generates from `BRIDGE_MODULES`
//! (`kinds::js::prelude`), never from this file.
//!
//! `--check` parses the tool source without running it and prints one
//! envelope (`{"ok": true}` or `{"ok": false, error, message, line, column,
//! source_line}`).

use std::io::{BufRead, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rquickjs::function::Func;
use rquickjs::promise::MaybePromise;
use rquickjs::{CatchResultExt, Ctx, Context, Function, Module, Object, Runtime, Value};
use serde_json::{Map, json};

const TOOL_FILE: &str = "tool.js";
/// Captured console output is capped to its tail, like python's runner.
const CAPTURE_CAP: usize = 200_000;
/// Appended to the tool module so a plain `function main` is reachable.
const MAIN_EPILOGUE: &str = "\n;globalThis.__mcphost_main = (typeof main === \"function\") ? main : undefined;\n";

struct Config {
    check: bool,
    memory_mb: usize,
    timeout_ms: u64,
    network: String,
    prelude: Option<String>,
    script: String,
}

fn parse_args() -> Config {
    let mut cfg = Config {
        check: false,
        memory_mb: 256,
        timeout_ms: 10_000,
        network: "none".to_string(),
        prelude: None,
        script: String::new(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--check" => cfg.check = true,
            "--memory-mb" => cfg.memory_mb = it.next().and_then(|v| v.parse().ok()).unwrap_or(256),
            "--timeout-ms" => cfg.timeout_ms = it.next().and_then(|v| v.parse().ok()).unwrap_or(10_000),
            "--network" => cfg.network = it.next().unwrap_or_else(|| "none".to_string()),
            "--prelude" => cfg.prelude = it.next(),
            _ => cfg.script = a,
        }
    }
    cfg
}

fn emit(v: &serde_json::Value) {
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(v.to_string().as_bytes());
    let _ = out.write_all(b"\n");
    let _ = out.flush();
}

/// `export default <expr>` becomes `var __mcphost_default = <expr>` on the
/// same line, so every line number the engine reports is the author's.
fn rewrite_export_default(source: &str) -> String {
    source
        .lines()
        .map(|l| match l.strip_prefix("export default") {
            Some(rest) if rest.starts_with(char::is_whitespace) || rest.starts_with('(') => {
                format!("globalThis.__mcphost_default ={rest}")
            }
            _ => l.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn tool_source(path: &str) -> String {
    rewrite_export_default(&std::fs::read_to_string(path).unwrap_or_default())
}

/// `tool.js:LINE[:COL]` out of the first stack frame that names the tool.
fn stack_position(stack: &str) -> Option<(i64, Option<i64>)> {
    for frame in stack.lines() {
        let Some(at) = frame.find("tool.js:") else { continue };
        let mut parts = frame[at + "tool.js:".len()..].split(|c: char| !c.is_ascii_digit());
        let line = parts.next()?.parse().ok()?;
        let col = parts.next().and_then(|c| c.parse().ok());
        return Some((line, col));
    }
    None
}

fn source_line(source: &str, line: i64) -> Option<String> {
    let n = usize::try_from(line).ok()?.checked_sub(1)?;
    source.lines().nth(n).map(|l| l.trim().to_string())
}

/// What the engine threw, as the pieces the envelope carries.
struct Thrown {
    name: String,
    message: String,
    stack: String,
}

fn describe_thrown<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Thrown {
    let obj = value.as_object().cloned();
    let get = |k: &str| obj.as_ref().and_then(|o| o.get::<_, Option<String>>(k).ok().flatten());
    let message = get("message").unwrap_or_else(|| {
        ctx.json_stringify(value.clone())
            .ok()
            .flatten()
            .and_then(|s| s.to_string().ok())
            .unwrap_or_else(|| "non-error value thrown".to_string())
    });
    Thrown {
        name: get("name").unwrap_or_else(|| "Error".to_string()),
        message,
        stack: get("stack").unwrap_or_default(),
    }
}

fn thrown_envelope(t: &Thrown, source: &str, interrupted: bool) -> serde_json::Value {
    if interrupted {
        return json!({"ok": false, "kind": "timeout"});
    }
    if t.message == "out of memory" {
        return json!({"ok": false, "kind": "oom"});
    }
    let mut env = json!({
        "ok": false, "kind": "exception", "error": t.name, "message": t.message,
        "traceback": t.stack,
    });
    if let Some((line, col)) = stack_position(&t.stack) {
        env["line"] = json!(line);
        if let Some(c) = col {
            env["column"] = json!(c);
        }
        if let Some(src) = source_line(source, line) {
            env["source_line"] = json!(src);
        }
    }
    env
}

fn check(cfg: &Config) {
    let source = tool_source(&cfg.script);
    let rt = Runtime::new().expect("runtime");
    rt.set_memory_limit(cfg.memory_mb * 1024 * 1024);
    let context = Context::full(&rt).expect("context");
    let envelope = context.with(|ctx| {
        match Module::declare(ctx.clone(), TOOL_FILE, source.clone()).catch(&ctx) {
            Err(e) => {
                let t = match e {
                    rquickjs::CaughtError::Exception(ex) => Thrown {
                        name: "SyntaxError".to_string(),
                        message: ex.message().unwrap_or_default(),
                        stack: ex.stack().unwrap_or_default(),
                    },
                    other => Thrown {
                        name: "SyntaxError".to_string(),
                        message: other.to_string(),
                        stack: String::new(),
                    },
                };
                let mut env = thrown_envelope(&t, &source, false);
                env["error"] = json!("SyntaxError");
                env
            }
            Ok(_) if has_main(&source) => json!({"ok": true}),
            Ok(_) => json!({"ok": false, "error": "NoMain", "message": "source does not define main(args)"}),
        }
    });
    emit(&envelope);
}

/// `main` is a function declaration, a binding, or the default export.
fn has_main(source: &str) -> bool {
    source.lines().any(|l| {
        let t = l.trim_start();
        let t = t.strip_prefix("export ").unwrap_or(t);
        let t = t.strip_prefix("async ").unwrap_or(t);
        t.starts_with("function main")
            || t.starts_with("function*main")
            || ["const main", "let main", "var main", "main ="]
                .iter()
                .any(|p| t.starts_with(p))
    }) || source.lines().any(|l| l.starts_with("globalThis.__mcphost_default ="))
}

fn read_line() -> Option<String> {
    let mut line = String::new();
    match std::io::stdin().lock().read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line),
    }
}

fn install_globals<'js>(
    ctx: &Ctx<'js>,
    cfg: &Config,
    out: &Arc<Mutex<String>>,
    err: &Arc<Mutex<String>>,
) -> rquickjs::Result<()> {
    let g = ctx.globals();
    // One blocking round trip: request line out, response line in.
    g.set(
        "__mcphost_bridge",
        Func::from(|req: String| -> String {
            emit(&serde_json::from_str(&req).unwrap_or(serde_json::Value::Null));
            read_line().unwrap_or_else(|| {
                r#"{"ok":false,"code":"bridge_closed","message":"the bridge channel closed"}"#.to_string()
            })
        }),
    )?;
    let console = Object::new(ctx.clone())?;
    for (name, buf) in [("log", out), ("info", out), ("debug", out), ("warn", err), ("error", err)] {
        let buf = buf.clone();
        console.set(
            name,
            Func::from(move |args: rquickjs::function::Rest<String>| {
                let mut b = buf.lock().unwrap_or_else(|e| e.into_inner());
                b.push_str(&args.0.join(" "));
                b.push('\n');
                if b.len() > CAPTURE_CAP * 2 {
                    let cut = b.len() - CAPTURE_CAP;
                    let cut = (cut..b.len()).find(|i| b.is_char_boundary(*i)).unwrap_or(b.len());
                    *b = b[cut..].to_string();
                }
            }),
        )?;
    }
    g.set("console", console)?;
    g.set("__mcphost_network", cfg.network.clone())?;
    let env: Map<String, serde_json::Value> = std::env::vars().map(|(k, v)| (k, json!(v))).collect();
    g.set("__mcphost_env", serde_json::Value::Object(env).to_string())?;
    Ok(())
}

fn caught_envelope<'js>(
    ctx: &Ctx<'js>,
    e: rquickjs::CaughtError<'js>,
    source: &str,
    interrupted: &AtomicBool,
) -> serde_json::Value {
    let was_interrupted = interrupted.load(Ordering::SeqCst);
    let t = match e {
        rquickjs::CaughtError::Exception(ex) => Thrown {
            name: ex.get::<_, Option<String>>("name").ok().flatten().unwrap_or_else(|| "Error".into()),
            message: ex.message().unwrap_or_default(),
            stack: ex.stack().unwrap_or_default(),
        },
        rquickjs::CaughtError::Value(v) => describe_thrown(ctx, v),
        rquickjs::CaughtError::Error(e) => Thrown { name: "Error".into(), message: e.to_string(), stack: String::new() },
    };
    thrown_envelope(&t, source, was_interrupted)
}

fn tail(s: &Arc<Mutex<String>>) -> String {
    let b = s.lock().unwrap_or_else(|e| e.into_inner());
    let start = (b.len().saturating_sub(CAPTURE_CAP)..b.len())
        .find(|i| b.is_char_boundary(*i))
        .unwrap_or(0);
    b[start..].to_string()
}

fn run_one(cfg: &Config, payload: &serde_json::Value, interrupted: &Arc<AtomicBool>, deadline: &Arc<Mutex<Instant>>) -> serde_json::Value {
    let t_start = Instant::now();
    *deadline.lock().unwrap_or_else(|e| e.into_inner()) = t_start + Duration::from_millis(cfg.timeout_ms);
    interrupted.store(false, Ordering::SeqCst);
    let source = tool_source(&cfg.script);
    let prelude = cfg.prelude.as_deref().map(|p| std::fs::read_to_string(p).unwrap_or_default()).unwrap_or_default();

    let rt = Runtime::new().expect("runtime");
    rt.set_memory_limit(cfg.memory_mb * 1024 * 1024);
    {
        let (interrupted, deadline) = (interrupted.clone(), deadline.clone());
        rt.set_interrupt_handler(Some(Box::new(move || {
            let over = Instant::now() > *deadline.lock().unwrap_or_else(|e| e.into_inner());
            if over {
                interrupted.store(true, Ordering::SeqCst);
            }
            over
        })));
    }
    let context = Context::full(&rt).expect("context");
    let (out, err) = (Arc::new(Mutex::new(String::new())), Arc::new(Mutex::new(String::new())));
    let args_json = payload.get("args").cloned().unwrap_or(json!({})).to_string();
    let end_user = payload.get("end_user").cloned().unwrap_or(serde_json::Value::Null).to_string();

    let (mut envelope, tool_ms) = context.with(|ctx| {
        let ctx: Ctx<'_> = ctx;
        let mut t_main = None;
        let mut run = || -> Result<serde_json::Value, serde_json::Value> {
            let caught = |e| caught_envelope(&ctx, e, &source, interrupted);
            install_globals(&ctx, cfg, &out, &err).map_err(|e| json!({"ok": false, "kind": "exception", "error": "Error", "message": e.to_string()}))?;
            ctx.globals().set("__mcphost_end_user", end_user.clone()).map_err(|e| json!({"ok": false, "kind": "exception", "error": "Error", "message": e.to_string()}))?;
            ctx.eval::<(), _>(prelude.clone()).catch(&ctx).map_err(&caught)?;
            let module = Module::declare(ctx.clone(), TOOL_FILE, format!("{source}{MAIN_EPILOGUE}")).catch(&ctx).map_err(&caught)?;
            let (_module, promise) = module.eval().catch(&ctx).map_err(&caught)?;
            promise.finish::<()>().catch(&ctx).map_err(&caught)?;
            let main: Option<Function> = ctx.globals().get("__mcphost_main").ok().filter(|f: &Option<Function>| f.is_some()).flatten();
            let main: Option<Function> = match main {
                Some(m) => Some(m),
                None => ctx.globals().get::<_, Option<Function>>("__mcphost_default").ok().flatten(),
            };
            let main = match main {
                Some(m) => m,
                None => {
                    return Err(json!({"ok": false, "kind": "exception", "error": "AttributeError", "message": "tool module has no main(args)", "traceback": ""}));
                }
            };
            let args = ctx.json_parse(args_json.clone()).catch(&ctx).map_err(&caught)?;
            t_main = Some(Instant::now());
            let returned: MaybePromise = main.call((args,)).catch(&ctx).map_err(&caught)?;
            let value: Value = returned.finish().catch(&ctx).map_err(&caught)?;
            if value.is_undefined() {
                return Ok(serde_json::Value::Null);
            }
            match ctx.json_stringify(value).catch(&ctx).map_err(&caught)? {
                Some(s) => serde_json::from_str(&s.to_string().unwrap_or_default())
                    .map_err(|e| json!({"ok": false, "kind": "output_invalid", "message": e.to_string()})),
                None => Err(json!({"ok": false, "kind": "output_invalid", "message": "tool result is not JSON-serializable"})),
            }
        };
        let r = run();
        let tool_ms = t_main.map(|t| t.elapsed().as_secs_f64() * 1000.0).unwrap_or(0.0);
        (match r { Ok(v) => json!({"ok": true, "result": v}), Err(e) => e }, tool_ms)
    });
    envelope["stdout_capture"] = json!(tail(&out));
    envelope["stderr_capture"] = json!(tail(&err));
    envelope["timings"] = json!({
        "args_ms": 0.0,
        "tool_code_ms": tool_ms,
        "serialize_ms": 0.0,
    });
    envelope
}

fn main() {
    let cfg = parse_args();
    if cfg.check {
        check(&cfg);
        return;
    }
    let interrupted = Arc::new(AtomicBool::new(false));
    let deadline = Arc::new(Mutex::new(Instant::now() + Duration::from_secs(3600)));
    while let Some(line) = read_line() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<serde_json::Value>(line) {
            Ok(payload) => emit(&run_one(&cfg, &payload, &interrupted, &deadline)),
            Err(e) => emit(&json!({"ok": false, "kind": "exception", "error": "PayloadError", "message": e.to_string(), "stdout_capture": "", "stderr_capture": ""})),
        }
    }
}
