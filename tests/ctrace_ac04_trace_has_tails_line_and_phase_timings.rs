//! PRD-mcphost-call-trace-replay
//! AC4 -- Given a python tool that prints 10 KiB and raises at `tool.py:16`,
//! When `host.call_trace` reads it, Then `stdout_tail` and `stderr_tail` are
//! each <= 4 KiB ending with the last bytes written, `line` is 16,
//! `source_line` is set, and `timings` has the four runner phases plus
//! `queue_ms` whose sum is within 10 ms of `duration_ms`.

use crate::ctrace;

use mcphost::sandbox;
use serde_json::{Value, json};

/// A tool whose `raise` is on line 16 of `tool.py`, after 10 KiB written to
/// stdout and 10 KiB to stderr.
fn source() -> String {
    let lines = [
        "import sys",                                  // 1
        "",                                            // 2
        "# padding so the raise below is on line 16",  // 3
        "",                                            // 4
        "",                                            // 5
        "",                                            // 6
        "",                                            // 7
        "",                                            // 8
        "def main(args):",                             // 9
        "    for i in range(160):",                    // 10
        "        print('%03d ' % i + 'x' * 59)",       // 11  64 bytes a line = 10 KiB
        "        print('%03d ' % i + 'e' * 59, file=sys.stderr)", // 12
        "    print('STDOUT-END')",                     // 13
        "    print('STDERR-END', file=sys.stderr)",    // 14
        "",                                            // 15
        "    raise RuntimeError('boom')",              // 16
    ];
    lines.join("\n") + "\n"
}

fn timing(trace: &Value, key: &str) -> f64 {
    trace["timings"][key].as_f64().unwrap_or_else(|| panic!("timings.{key} missing: {trace}"))
}

#[tokio::test]
async fn trace_of_a_raising_tool_has_bounded_tails_the_line_and_timings_that_add_up() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let _switch = ctrace::trace_switch().await;
    let fx = ctrace::start().await;
    fx.publish_python("noisy", &source()).await;
    fx.warm("noisy", json!({})).await;

    // The first call after the build is a cold one; the second is warm. The
    // phases must account for the duration on both paths.
    for path in ["cold", "warm"] {
        let err = fx.tool_call("noisy", json!({})).await.expect_err("the tool raises");
        let call_id = err.data["call_id"].as_str().unwrap_or_else(|| panic!("{path}: call_id in {err:?}")).to_string();
        let trace = fx.trace(&fx.client, &call_id).await.expect("host.call_trace");

        let stdout = trace["stdout_tail"].as_str().expect("stdout_tail");
        let stderr = trace["stderr_tail"].as_str().expect("stderr_tail");
        assert!(stdout.len() <= 4096, "{path}: stdout_tail is {} bytes", stdout.len());
        assert!(stderr.len() <= 4096, "{path}: stderr_tail is {} bytes", stderr.len());
        assert!(stdout.len() > 4000, "{path}: stdout_tail keeps the tail, not a stub: {} bytes", stdout.len());
        assert!(stdout.ends_with("STDOUT-END\n"), "{path}: stdout_tail ends with the last bytes written: {:?}", &stdout[stdout.len() - 40..]);
        assert!(
            stderr.trim_end().ends_with("RuntimeError: boom"),
            "{path}: stderr_tail ends with the traceback's last line: {:?}",
            &stderr[stderr.len() - 60..]
        );
        assert!(stderr.contains("STDERR-END"), "{path}: the last stderr write precedes the traceback");

        assert_eq!(trace["error_code"], json!("tool_exception"), "{path}: {trace}");
        assert_eq!(trace["line"], json!(16), "{path}: {trace}");
        let source_line = trace["source_line"].as_str().unwrap_or_else(|| panic!("{path}: source_line set: {trace}"));
        assert!(source_line.contains("raise RuntimeError"), "{path}: source_line {source_line:?}");

        let phases = ["queue_ms", "runner_start_ms", "args_ms", "tool_code_ms", "serialize_ms"];
        let sum: f64 = phases.iter().map(|k| timing(&trace, k)).sum();
        let id: i64 = call_id.parse().expect("numeric call_id");
        let duration_ms = fx
            .server
            .state
            .db
            .call_duration_ms_for_test(id)
            .await
            .expect("duration lookup")
            .expect("the calls row exists") as f64;
        assert!(
            (sum - duration_ms).abs() <= 10.0,
            "{path}: phases {:?} sum to {sum} ms but duration_ms is {duration_ms}: {}",
            phases.iter().map(|k| (*k, timing(&trace, k))).collect::<Vec<_>>(),
            trace["timings"]
        );
    }
}
