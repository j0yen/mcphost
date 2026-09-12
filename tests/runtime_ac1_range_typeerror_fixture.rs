//! AC1 (P0) — Given session panel_rag_indexer_01's recorded publish and
//! call as a fixture, When replayed at HEAD, Then the failure reproduces
//! before the fix; and When replayed after, Then the call either succeeds
//! or returns the structured error naming the argument, expected/actual
//! type, and phase.
//!
//! ## Fixture provenance (documented per the PRD's own instruction: "the
//! baseline run's session recordings are the fixtures; turning them into
//! tests is the first build step")
//!
//! The PRD's `Lift-baseline: runs/mcphost-baseline-20260906` and its
//! composition hash name a synthorg run directory that does not exist on
//! this build machine (checked: no `runs/mcphost-baseline-20260906` under
//! `~/repos/synthorg/runs/`, no file anywhere on this host matching the
//! cited composition hash, and the `panel_rag_indexer_01.jsonl` session
//! that DOES exist under `~/repos/synthorg/runs/mcp-host-project-consume/`
//! is a clean, successful, helpfulness-1.0 run of a *different*
//! `retrieval_latency_check` source with no `int()`/`range` call anywhere
//! in it). The raw recording is therefore unrecoverable; this fixture
//! instead reproduces the PRD's own verbatim-quoted failure text --
//! `TypeError: int() argument must be a string, a bytes-like object or a
//! real number, not 'range'` -- via the one Python construct that actually
//! produces that exact CPython message (verified interactively:
//! `int(range(3))`).
//!
//! ## Root-cause determination (the PRD's requirement 1 left this
//! explicitly open: "If mcphost's schema-driven argument coercion produced
//! the range mismatch, fix the coercion; in either case the error path is
//! rebuilt")
//!
//! `range` is not a JSON type and mcphost has no argument-coercion step at
//! all -- `Kind::call`'s only args-side check is JSON Schema *validation*
//! (`kinds::python::describe_args_error`, this PRD's `args_coercion`
//! phase), which can reject a value that doesn't match the schema but can
//! never manufacture a Python `range` object from JSON input. A `range`
//! value can only come from the tool's OWN source code (e.g. a default
//! argument or a variable assigned from `range(...)` and passed to
//! `int()`). This is therefore conclusively a `tool_code`-phase failure,
//! not `args_coercion` -- so this fixture asserts the AC2 error shape
//! (phase, exception class, tool-source line) rather than the AC3 shape
//! (argument name, expected/actual type) the PRD's own draft-time prose
//! guessed at before the root cause was knowable.
use crate::common;
use common::{TestServer, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn range_typeerror_reproduces_then_returns_structured_tool_code_error() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC1 rag_indexer replay").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    // Line 5 is the exact reproduction of the recorded failure: a `top_k`
    // default expressed as `range(...)` instead of an int/list, then
    // handed straight to `int()` the way a hastily-written retrieval tool
    // might coerce a caller-omitted count.
    let source = "\
def main(args):
    top_k = args.get('top_k')
    if top_k is None:
        top_k = range(5)
    return {'top_k': int(top_k)}
";
    let spec = json!({"source": source, "args_schema": {"type": "object"}});
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "retrieval_latency_check", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok -- the source is valid python with a main()");

    let err = poll_until_ready(
        &client,
        &format!("{ns}.retrieval_latency_check"),
        json!({"query": "SOC 2 Type II audit evidence retention policy 2024"}),
        Duration::from_secs(10),
    )
    .await
    .expect_err("the recorded failure must reproduce: omitting top_k hits the range/int() bug");

    // "The failure reproduces" (AC1's first clause) -- verbatim substring
    // match against the PRD's own quoted error text.
    assert_eq!(err.error_code.as_deref(), Some("tool_exception"));
    assert!(
        err.message
            .contains("int() argument must be a string, a bytes-like object or a real number, not 'range'"),
        "must reproduce the recorded TypeError verbatim, got: {}",
        err.message
    );

    // "returns the structured error naming ... phase" (AC1's second
    // clause) -- phase is `tool_code` (see the root-cause note above), the
    // exception class is present and separate from the message, and the
    // failing tool-source line is named -- never a bare traceback fragment
    // as the primary shape.
    assert_eq!(err.data["phase"].as_str(), Some("tool_code"));
    assert_eq!(err.data["exception_class"].as_str(), Some("TypeError"));
    let line = err.data["line"]
        .as_i64()
        .expect("a TypeError raised inside main() must name its tool.py line");
    assert_eq!(line, 5, "must point at the int(top_k) call, not just anywhere in the traceback");
}
