//! Shared harness for the `mrefu_ac*` proofs
//! (PRD-mcphost-migration-refusal-contract): a live database seeded by the
//! real `mcphost migrate`, a fixture migration directory for
//! `--extra-migrations-dir`, a stub "previous release" that answers every
//! check-compat probe except one, and a runner that returns the CLI's exit
//! code, stdout and the parsed `--report` document.
//!
//! Pulled in per-file with `#[path = "support/mrefu.rs"] mod mrefu;`.

#![allow(dead_code)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

pub fn mcphost_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_mcphost"))
}

pub fn scratch_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-mrefu-{tag}-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
    ));
    fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

/// A live database migrated by the real binary, the way `serve` would on
/// first start. Returns the sqlite file.
pub fn seed_live_db(dir: &Path) -> PathBuf {
    let data = dir.join("data");
    fs::create_dir_all(&data).expect("create data dir");
    let status = Command::new(mcphost_bin())
        .arg("migrate")
        .env("MCPHOST_DATA_DIR", &data)
        .status()
        .expect("run mcphost migrate");
    assert!(status.success(), "seeding migrate should succeed");
    data.join("mcphost.db")
}

/// Run `sql` against the live file (fixture tables the real migrations do
/// not know about).
pub fn exec_sql(db: &Path, sql: &str) {
    let conn = rusqlite::Connection::open(db).expect("open live db");
    conn.execute_batch(sql).expect("fixture sql");
}

/// A directory holding one fixture migration file.
pub fn fixture_migration(dir: &Path, file: &str, sql: &str) -> PathBuf {
    let fx = dir.join("fixture-migrations");
    fs::create_dir_all(&fx).expect("create fixture dir");
    fs::write(fx.join(file), sql).expect("write fixture migration");
    fx
}

/// The stub previous release. `--version` prints `mcphost 0.106.0`; as
/// `serve` it answers `/healthz` (with the compat token), `initialize`,
/// `tools/list` and the throwaway `signup`, and fails the `host.whoami`
/// `tools/call` the way a release whose queries name a dropped column
/// would. The listener arrives as the inherited fd 3.
pub fn stub_previous_failing_tools_call(dir: &Path) -> PathBuf {
    const STUB: &str = r#"#!/usr/bin/env python3
import json, os, socket, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
if sys.argv[1:] == ["--version"]:
    print("mcphost 0.106.0")
    sys.exit(0)
TOKEN = os.environ.get("MCPHOST_COMPAT_TOKEN", "")

class H(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def _json(self, obj):
        body = json.dumps(obj).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        self.send_response(200)
        self.send_header("X-Mcphost-Compat-Token", TOKEN)
        self.send_header("Content-Length", "2")
        self.end_headers()
        self.wfile.write(b"ok")

    def do_POST(self):
        req = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
        rid = req.get("id")
        params = req.get("params") or {}
        if req.get("method") == "tools/call" and params.get("name") == "host.whoami":
            self._json({"jsonrpc": "2.0", "id": rid,
                        "error": {"code": -32603, "message": "no such column fixture_t.b"}})
        elif req.get("method") == "tools/call":
            self._json({"jsonrpc": "2.0", "id": rid,
                        "result": {"structuredContent": {"key": "stub-key"}}})
        else:
            self._json({"jsonrpc": "2.0", "id": rid, "result": {}})

srv = HTTPServer(("127.0.0.1", 0), H, bind_and_activate=False)
srv.socket = socket.socket(fileno=3)
srv.serve_forever()
"#;
    let path = dir.join("previous-stub.py");
    fs::write(&path, STUB).expect("write stub");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod stub");
    path
}

pub struct CheckRun {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub report_path: PathBuf,
}

impl CheckRun {
    pub fn report(&self) -> Value {
        let text = fs::read_to_string(&self.report_path).unwrap_or_else(|e| {
            panic!("report {} unreadable: {e}\nstdout: {}\nstderr: {}", self.report_path.display(), self.stdout, self.stderr)
        });
        serde_json::from_str(&text).expect("report is JSON")
    }
}

/// `mcphost migrate --check-compat --previous <prev> --db <db> --report
/// <dir>/r.json [--extra-migrations-dir <fx>]`.
pub fn run_check(dir: &Path, db: &Path, previous: &Path, fixture: Option<&Path>) -> CheckRun {
    let report_path = dir.join("r.json");
    let mut cmd = Command::new(mcphost_bin());
    cmd.arg("migrate")
        .arg("--check-compat")
        .arg("--previous")
        .arg(previous)
        .arg("--db")
        .arg(db)
        .arg("--report")
        .arg(&report_path);
    if let Some(fx) = fixture {
        cmd.arg("--extra-migrations-dir").arg(fx);
    }
    let out = cmd.output().expect("run mcphost migrate --check-compat");
    CheckRun {
        code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        report_path,
    }
}

/// The committed `docs/compat-report.schema.json`.
pub fn committed_schema() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/compat-report.schema.json");
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} unreadable: {e}", path.display()));
    serde_json::from_str(&text).expect("committed schema is JSON")
}

/// Panic naming every schema violation of `doc`.
pub fn assert_valid_against_committed_schema(doc: &Value) {
    let schema = committed_schema();
    let validator = jsonschema::validator_for(&schema).expect("committed schema compiles");
    let errors: Vec<String> = validator.iter_errors(doc).map(|e| format!("{e} at {}", e.instance_path)).collect();
    assert!(errors.is_empty(), "report violates docs/compat-report.schema.json: {errors:?}\n{doc}");
}
