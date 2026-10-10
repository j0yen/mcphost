//! Shared fixture for the `xlinks_ac*` tests (PRD-mcphost-docs-external-
//! links-resolve): a loopback HTTP server with fixed routes, and a runner
//! for the real `scripts/docs-link-check.sh` against scratch files. Nothing
//! here touches the network beyond 127.0.0.1.
#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Binds 127.0.0.1:0 and serves forever on background threads. Routes:
/// `/ok` 200 (a `?query` is ignored, so one route serves many distinct URLs); `/gone` 404; `/head405` HEAD->405, GET->200; `/hang` reads
/// the request and never answers; anything else 404.
pub fn serve() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || handle(stream));
        }
    });
    port
}

fn handle(mut stream: std::net::TcpStream) {
    let mut buf = [0u8; 4096];
    let n = stream.read(&mut buf).unwrap_or(0);
    let head = String::from_utf8_lossy(&buf[..n]).to_string();
    let mut parts = head.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");
    let path = path.split('?').next().unwrap_or(path);
    let status = match (method, path) {
        (_, "/ok") => "200 OK",
        ("HEAD", "/head405") => "405 Method Not Allowed",
        (_, "/head405") => "200 OK",
        (_, "/hang") => {
            std::thread::sleep(std::time::Duration::from_secs(30));
            return;
        }
        _ => "404 Not Found",
    };
    let body = if method == "HEAD" { "" } else { "x" };
    let resp = format!("HTTP/1.1 {status}\r\nContent-Length: 1\r\nConnection: close\r\n\r\n{body}");
    let _ = stream.write_all(resp.as_bytes());
}

pub fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-xlinks-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Runs the real checker over `files` with the real allowlist and a 1 s
/// timeout (the script's default is 10 s; `DOCS_LINK_TIMEOUT` shortens it
/// so the timeout case does not make the suite slow).
pub fn run_checker(files: &[&Path]) -> Output {
    Command::new(repo_root().join("scripts/docs-link-check.sh"))
        .args(files)
        .env("DOCS_LINK_TIMEOUT", "1")
        .output()
        .expect("run docs-link-check.sh")
}
