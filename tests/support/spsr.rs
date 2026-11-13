//! PRD-mcphost-sandbox-pool-self-restart-v2: the shared fixture of the
//! `spsr_ac*` tests -- a real python kind with a warm pool, a tool that wedges
//! (a warm child stopped with SIGSTOP and a call whose arguments exceed the
//! pipe buffer, so the call blocks in its stdin write), and healthy calls.
//!
//! Not a crate module: pulled in by `use crate::spsr;` (the suite generator
//! adds the `#[path]` declaration).

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use mcphost::kinds::python::PythonKind;
use mcphost::kinds::{CallCtx, Kind, KindError};

pub const TENANT: i64 = 71;
pub const NAMESPACE: &str = "spsr";

/// The stalled tool's flight key.
pub const STALLED_KEY: &str = "71/stalled_tool";

/// Bytes of argument payload: well past the 64 KiB pipe buffer.
pub const BIG_ARGS_BYTES: usize = 512 * 1024;

pub fn data_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mcphost-spsr-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub fn ctx(tool: &str) -> CallCtx {
    let mut ctx = CallCtx::for_test(TENANT, NAMESPACE);
    ctx.tool_name = Some(tool.to_string());
    ctx
}

/// A tool that returns its `n` doubled, after sleeping `sleep_s` seconds.
pub fn tool_spec(timeout_s: u64) -> Value {
    json!({
        "source": "import time\ndef main(args):\n    time.sleep(float(args.get(\"sleep_s\", 0)))\n    return {\"n\": args.get(\"n\", 0) * 2}\n",
        "args_schema": {"type": "object"},
        "requirements": [],
        "timeout_s": timeout_s,
    })
}

pub fn big_args() -> Value {
    json!({"n": 1, "blob": "x".repeat(BIG_ARGS_BYTES)})
}

/// Calls until the environment build is over (the first call of a spec may
/// answer `building`).
pub async fn call_ready(kind: &PythonKind, spec: &Value, args: Value, ctx: &CallCtx) -> Result<Value, KindError> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let out = kind.call(spec, args.clone(), ctx).await;
        let building = match &out {
            Err(KindError::Structured { code, .. }) => *code == "tool_building",
            Ok(v) => v.get("status").and_then(Value::as_str) == Some("building"),
            _ => false,
        };
        if !building || Instant::now() > deadline {
            return out;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// A python kind with a warm child for `tool` (the first call seeds the pool).
pub async fn kind_with_warm_child(tag: &str, tool: &str, spec: &Value) -> Arc<PythonKind> {
    let kind = Arc::new(PythonKind::for_test_with_warm_pool(&data_dir(tag), Duration::from_secs(120), 4, 8));
    call_ready(&kind, spec, json!({"n": 1}), &ctx(tool)).await.expect("seed call");
    wait_warm(&kind, 1).await;
    // The second call runs in the warm child itself: its interpreter is up and
    // reading, so stopping the child's tree stalls it for real.
    call_ready(&kind, spec, json!({"n": 1}), &ctx(tool)).await.expect("warm call");
    wait_warm(&kind, 1).await;
    kind
}

pub async fn wait_warm(kind: &PythonKind, want: usize) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while kind.warm_metrics().2 < want {
        assert!(Instant::now() < deadline, "warm pool never reached {want}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Every descendant of `root` (children, grandchildren, ...), read from
/// `/proc`. The sandbox is bwrap inside bwrap: the inner processes sit in
/// process groups of their own, so a `killpg` of the child's group never
/// reaches the interpreter.
pub fn descendants(root: i32) -> Vec<i32> {
    let mut parent_of = std::collections::HashMap::new();
    for entry in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<i32>() else { continue };
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else { continue };
        let Some(rest) = stat.rsplit(") ").next() else { continue };
        if let Some(ppid) = rest.split_whitespace().nth(1).and_then(|p| p.parse::<i32>().ok()) {
            parent_of.insert(pid, ppid);
        }
    }
    let mut found = vec![root];
    let mut i = 0;
    while i < found.len() {
        let at = found[i];
        found.extend(parent_of.iter().filter(|(_, p)| **p == at).map(|(c, _)| *c));
        i += 1;
    }
    found.split_off(1)
}

/// SIGSTOPs every idle warm child -- its process group leader and every
/// descendant, the interpreter included -- and returns the leaders' pgids.
pub fn stop_warm_children(kind: &PythonKind) -> Vec<i32> {
    let pids = kind.warm_child_pids_for_test();
    for pid in &pids {
        for victim in std::iter::once(*pid).chain(descendants(*pid)) {
            // SAFETY: `victim` is a process of the sandbox this kind spawned.
            unsafe {
                libc::kill(victim, libc::SIGSTOP);
            }
        }
    }
    pids
}

/// `true` while process group `pgid` still has a member.
pub fn group_alive(pgid: i32) -> bool {
    // SAFETY: signal 0 only probes.
    unsafe { libc::killpg(pgid, 0) == 0 }
}

/// Waits up to 5 s for every pid in `pids` to be gone.
pub async fn all_gone(pids: &[i32]) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        // SAFETY: signal 0 only probes.
        let alive = pids.iter().any(|p| unsafe { libc::kill(*p, 0) } == 0 && !is_zombie(*p));
        if !alive {
            return true;
        }
        if Instant::now() > deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn is_zombie(pid: i32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|st| st.rsplit(") ").next().and_then(|r| r.chars().next()))
        == Some('Z')
}

pub fn is_tool_timeout(err: &KindError) -> bool {
    matches!(err, KindError::Structured { code, .. } if *code == "tool_timeout")
}

// ---- app-level rig (AC2 onward) --------------------------------------------

use mcphost::heal::HealRow;
use mcphost::signals::{HealConfig, Signal, Signals};
use mcphost::state::AppState;

use crate::common;

/// The wedge factor the rig's `heal.toml` uses: a 3 s flight is wedged at 0.24 s.
pub const RIG_WEDGE_FACTOR: f64 = 0.08;

/// An `AppState` whose signals use [`RIG_WEDGE_FACTOR`] and a python kind with
/// a warm pool; `drain_budget_s` is `heal.toml`'s `drain_budget_s`.
pub async fn app(drain_budget_s: u64) -> (AppState, Arc<PythonKind>, common::TempDataDir) {
    let (mut state, dir) = common::bare_app_state().await;
    let toml = mcphost::signals::COMPILED_HEAL_TOML
        .replace("wedge_factor = 3.0", &format!("wedge_factor = {RIG_WEDGE_FACTOR}"))
        .replace("drain_budget_s = 30", &format!("drain_budget_s = {drain_budget_s}"));
    state.signals = Signals::new(HealConfig::parse(&toml).expect("rig heal.toml parses"));
    let python = Arc::new(PythonKind::for_test_with_warm_pool(&dir.0, Duration::from_secs(120), 4, 8));
    state.kinds.register(python.clone());
    (state, python, dir)
}

/// A stalled call in flight: its tool's warm child stopped, a call with
/// arguments past the pipe buffer blocked in its stdin write.
pub struct Wedge {
    pub pgid: i32,
    /// The stopped child's descendants (the interpreter among them).
    pub tree: Vec<i32>,
    pub call: tokio::task::JoinHandle<Result<Value, KindError>>,
    pub _cleanup: GroupGuard,
}

/// Kills the group on drop: a test that ends with its child still stopped
/// must not leak it.
pub struct GroupGuard(i32);

impl Drop for GroupGuard {
    fn drop(&mut self) {
        // SAFETY: the pgid is the sandbox process-group leader this rig stopped.
        unsafe {
            libc::killpg(self.0, libc::SIGKILL);
        }
    }
}

/// Wedges one flight on `tool` (timeout 1 s + 2) and waits until the detector
/// would name it.
pub async fn wedge(kind: &Arc<PythonKind>, tool: &str) -> Wedge {
    wedge_with(kind, tool, tool_spec(1)).await
}

/// [`wedge`] for a caller-supplied spec (`timeout_s` 1).
pub async fn wedge_with(kind: &Arc<PythonKind>, tool: &str, spec: Value) -> Wedge {
    call_ready(kind, &spec, json!({"n": 1}), &ctx(tool)).await.expect("seed call");
    wait_warm(kind, 1).await;
    // A second call runs in the warm child itself, so whatever the tool writes
    // to stderr is in that child's tail when it is stopped.
    call_ready(kind, &spec, json!({"n": 1}), &ctx(tool)).await.expect("warm call");
    wait_warm(kind, 1).await;
    let pgid = *stop_warm_children(kind).first().expect("a warm child");
    let tree = descendants(pgid);
    assert!(!tree.is_empty(), "the sandbox has an interpreter below its bwrap");
    let call = {
        let (kind, tool) = (kind.clone(), tool.to_string());
        tokio::spawn(async move { call_ready(&kind, &spec, big_args(), &ctx(&tool)).await })
    };
    tokio::time::sleep(Duration::from_millis(700)).await;
    let pool = Kind::pool_signals(kind.as_ref(), RIG_WEDGE_FACTOR).expect("pool");
    if call.is_finished() {
        panic!("the stalled call ended early: {:?}", call.await);
    }
    assert_eq!(pool.wedged, 1, "the stalled call is wedged: {pool:?}");
    Wedge { pgid, tree, call, _cleanup: GroupGuard(pgid) }
}

/// Feeds the signals two red samples `for_s` apart: one `Breach` for
/// `sandbox_pool`, which the subscriber receives.
pub fn breach(state: &AppState) {
    let t0 = mcphost::state::now_unix();
    state.signals.record(Signal::SandboxPool, t0, 1.0);
    let ev = state.signals.record(Signal::SandboxPool, t0 + 120, 1.0);
    assert!(ev.is_some(), "two red samples 120 s apart breach");
}

/// The newest `sandbox_pool_restart` row once one exists (any outcome, dry run
/// included).
pub async fn wait_row(state: &AppState) -> HealRow {
    wait_row_after(state, 0).await
}

/// The newest `sandbox_pool_restart` row once more than `have` exist.
pub async fn wait_row_after(state: &AppState, have: i64) -> HealRow {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let rows = state
            .db
            .list_heal_rows_since(0, Some("sandbox_pool_restart".to_string()), 100)
            .await
            .expect("list rows");
        if rows.len() as i64 > have {
            return rows.into_iter().next().expect("newest row");
        }
        assert!(Instant::now() < deadline, "no sandbox_pool_restart row appeared");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
